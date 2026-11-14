//! Sketch mode's ten constraint tools, headless: Horizontal, Vertical, Coincident, Tangent,
//! Equal, Parallel, Perpendicular, Midpoint, Symmetric, and Fix (Fable's decisions C1 to C9
//! of 2026-11-14, open item 34 of docs/design/m5.0-app.md, on the study
//! docs/study/constraint-tools.md). Each is a click on a selection made first (C7), which
//! [`relate`] turns into one edit, from the newest `sketch` read's members and the selection
//! in the order it was picked (C9: the sketch's points, lines, circles, and arcs, the origin,
//! and the axes). No viewport, no session: the app sends the edit, and shows the refusal or
//! the message.
//!
//! - **What it checks** is only what it needs to choose a form and its roles: what each
//!   member picked is (a point, the origin among them; a line or an axis; a circle or an
//!   arc), and how many. A selection it can't read is refused with the tool's one sentence
//!   ([`RelationTool::takes`]), the selection as the error's elements. Everything else wrong
//!   with a selection (a curve's own point, only the origin and the axes, a line made equal
//!   to a circle, a mirror among its own points) is the gate's to refuse, in its words, when
//!   the command runs.
//! - **The edit** is a `sketch_add` of the constraints written, applied whatever the solve
//!   finds (SD7), but in two places where the document's structure is the relation:
//!   - Coincident of two or more points is a `sketch_merge` of them in pick order, the first
//!     picked surviving (C1), refused where its solve is worse, as every merge is;
//!   - a `tangent_on` or a `midpoint` that implies an `on` the sketch holds replaces it: the
//!     add, then a `sketch_remove` of the `on` (C5), one undo step for the app.
//! - **Roles.** `a` is the first picked and `b` the second wherever the structure doesn't
//!   fix them. Several members are a star, each after the first paired with the first (C8),
//!   the origin as any other.
//! - **Already there** (C6): a constraint the click would write is left out where the sketch
//!   holds one equal to it in everything but its ID, `a` and `b` either way round in the
//!   forms where they're interchangeable. With every one left out, nothing is sent, and
//!   [`Related::message`] says so; with some, it counts them.
//! - **Read from the drawing,** the newest read's coordinates: a tangent's `side`, a
//!   `tangent_at`'s or `tangent_on`'s `direction` (C3, by `sketch_edit::drawn` and
//!   `sketch_edit::leaving`, trim's own), and Fix's values, written by Smart Dimension's own
//!   writer and rounded to the nanometre (C2).

use std::collections::BTreeMap;

use super::build::Builder;
use super::dimension::{Axis, along};
use super::geometry::{cross, distance, dot, sub};
use super::infer::{View, at_origin};
use super::{NewIds, Xy};
use crate::document::sketch_edit::{self, Geo, Members};
use crate::document::{
    Direction, ElementId, ElementKind, Form, Reference, Sketch, SketchConstraint, TangentSide,
};
use crate::{Error, ErrorCode};

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
            RelationTool::Tangent => "Tangent takes two curves, at least one a circle or an arc",
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

/// The edit a click makes, for the app to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelateEdit {
    /// A `sketch_add` of these constraints.
    Add(Members),
    /// A `sketch_merge` of these points, in pick order, the first surviving (C1).
    Merge(Vec<ElementId>),
    /// A `sketch_add` of `add`, then, once it's taken, a `sketch_remove` of the `on`s it
    /// implies (C5): one undo step. If the add is refused, nothing else is sent.
    AddThenRemove {
        add: Members,
        remove: Vec<ElementId>,
    },
}

/// What a click on a constraint tool makes ([`relate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    /// The edit to send: none where every constraint the click would write is in the sketch
    /// already (C6).
    pub edit: Option<RelateEdit>,
    /// The sketch's constraints that are those the click would write, left out (C6), by ID,
    /// in the order the click would write them.
    pub already: Vec<ElementId>,
    /// What the app's toast says: none where nothing was left out.
    pub message: Option<String>,
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
        already: String,
    },
    Merge(Vec<ElementId>),
}

/// A place two curves can be tangent at (C3): an end they share, or an end of `curve` held
/// on `on` by the `on`s `ons`, or by a `tangent_on`, which holds its end on its `b` as an `on`
/// does (`ons` empty where only a `tangent_on` holds it: then there's no `on` to replace).
enum Place<'p> {
    Shared,
    Held {
        curve: &'p Reference,
        on: &'p Reference,
        ons: Vec<ElementId>,
    },
}

/// The edit `relation`'s click makes of `picks`, the selection in the order it was picked,
/// in the sketch `read`, the newest `sketch` read's members as solved; its new constraints'
/// IDs from `ids`. Refused with the tool's sentence ([`RelationTool::takes`]) for a
/// selection it can't read, the selection as the error's elements; with
/// [`NOTHING_SELECTED`] for none; and, for Tangent, two lines (in the gate's words), a
/// round's centre on the line exactly, and more than one place the curves could be tangent
/// at with no point picked to say which (C3). Each tool, as C1 to C8 decide:
///
/// - **Horizontal, Vertical:** one `horizontal` (`vertical`) for each line, in pick order;
///   or, for two or more points, a `horizontal_points` (`vertical_points`) of each with the
///   first picked.
/// - **Coincident:** one or more points and one or more curves, an `on` for each point and
///   each curve, points in pick order, then curves (the origin a point, an axis a curve);
///   two or more points, a merge, the first surviving; a point and the origin, #278's A pair,
///   whichever was picked first. The origin with two or more points is refused.
/// - **Tangent:** two curves, and a point if it says which place. The places they can be
///   tangent at are the ends they share and the ends of one that an `on` or a `tangent_on`
///   holds on the other (so a second click where a `tangent_on` holds is C6's repeat):
///   none, an edge `tangent`, its `side` the side of the line the round's centre is drawn on
///   (an axis faces +x or +y), or for two rounds `outside` where their centres are at least
///   the larger radius apart, else `inside` (FreeCAD's rule); one, or the one picked, a
///   `tangent_at` there, `same` where `a` arriving and `b` leaving have a dot product of at
///   least 0, or a `tangent_on` of the curve that ends there, `same` by trim's rule, which
///   replaces an `on` holding that end, where one does (C5).
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
///   Dimension refuses it ([`Message::TooFar`](super::Message::TooFar)).
pub fn relate(
    read: &Sketch,
    relation: RelationTool,
    picks: &[Reference],
    ids: &mut dyn NewIds,
) -> Result<Related, Error> {
    let _span =
        tracing::debug_span!("relate", relation = relation.name(), picks = picks.len()).entered();
    if picks.is_empty() {
        return Err(Error::new(ErrorCode::InvalidInput, NOTHING_SELECTED));
    }
    let kinds = kinds(read);
    let view = View::of(read, &Members::default());
    let picked = Picked {
        read,
        kinds: &kinds,
        picks,
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
    let (forms, replaces, already_said) = match plan {
        Plan::Merge(points) => {
            return Ok(Related {
                edit: Some(RelateEdit::Merge(points)),
                already: Vec::new(),
                message: None,
            });
        }
        Plan::Constraints {
            forms,
            replaces,
            already,
        } => (forms, replaces, already),
    };
    // C6: each the sketch holds already left out.
    let mut new = Vec::new();
    let mut already = Vec::new();
    for form in forms {
        match read.constraints.iter().find(|c| repeats(&form, &c.form)) {
            Some(held) => already.push(held.id.clone()),
            None => new.push(form),
        }
    }
    if new.is_empty() {
        return Ok(Related {
            edit: None,
            already,
            message: Some(format!("{already_said}: nothing was added")),
        });
    }
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
    let message = match already.len() {
        0 => None,
        1 => Some("1 of these constraints was already there: it wasn't added again".to_owned()),
        n => Some(format!(
            "{n} of these constraints were already there: they weren't added again"
        )),
    };
    Ok(Related {
        edit: Some(edit),
        already,
        message,
    })
}

/// Whether `form` is `held` in everything but an ID (C6): equal, or, in the forms whose `a`
/// and `b` are interchangeable, equal with them swapped.
fn repeats(form: &Form, held: &Form) -> bool {
    form == held
        || match (form, held) {
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

    /// A refusal saying `message`, the selection as its elements.
    fn refusal(&self, message: impl Into<String>) -> Error {
        Error::new(ErrorCode::InvalidInput, message)
            .with_elements(self.picks.iter().map(|r| r.as_str().to_owned()))
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
    /// members of one kind, else each by its own noun, "point pe000007 and the origin".
    fn named(&self, references: &[&Reference]) -> String {
        let kinds: Vec<Option<ElementKind>> = references
            .iter()
            .map(|r| r.element().and_then(|id| self.kinds.get(id).copied()))
            .collect();
        match kinds.first() {
            Some(Some(first))
                if references.len() > 1 && kinds.iter().all(|k| *k == Some(*first)) =>
            {
                let ids: Vec<String> = references.iter().map(|r| r.as_str().to_owned()).collect();
                format!("{} {}", first.list(), listed(&ids))
            }
            _ => {
                let nouns: Vec<String> = references.iter().map(|r| self.noun(r)).collect();
                listed(&nouns)
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
                    if vertical {
                        Form::Vertical { line }
                    } else {
                        Form::Horizontal { line }
                    }
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
            already: format!("{} {} already {word}", self.named(&all), be(all.len())),
        })
    }

    /// Coincident: points and curves, an `on` of each pair; two or more points, a merge; a
    /// point and the origin, #278's A pair (C1).
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
            let forms = points
                .iter()
                .flat_map(|point| {
                    curves.iter().map(|curve| Form::On {
                        point: (*point).clone(),
                        curve: (*curve).clone(),
                    })
                })
                .collect();
            return Ok(Plan::Constraints {
                forms,
                replaces: Vec::new(),
                already: format!(
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
                    already: format!("{} is already at the origin", self.noun(&point)),
                    forms: at_origin(point).into(),
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
            already: format!("{} are already {word}", self.named(&all)),
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
            forms: vec![Form::Perpendicular {
                a: a.clone(),
                b: b.clone(),
            }],
            replaces: Vec::new(),
            already: format!("{} are already perpendicular", self.named(&[a, b])),
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
            forms: vec![Form::Midpoint {
                point: (*point).clone(),
                line: (*line).clone(),
            }],
            replaces: ons(self.read, point, line),
            already: format!(
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
            forms: vec![Form::Symmetric {
                a: a.clone(),
                b: b.clone(),
                about: about.clone(),
            }],
            replaces: Vec::new(),
            already: format!(
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
            for axis in [Axis::X, Axis::Y] {
                let form = along(axis, origin, (point, at))
                    .map_err(|message| self.refusal(message.text()))?;
                forms.push(form);
            }
        }
        let all: Vec<&Reference> = self.picks.iter().collect();
        Ok(Plan::Constraints {
            forms,
            replaces: Vec::new(),
            already: format!("{} {} already fixed", self.named(&all), be(all.len())),
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
            return Err(self.refusal(crate::document::TWO_LINES_TANGENT));
        }
        let places = self.places(a, b);
        let chosen = match points.first() {
            Some(point) => match point.element().and_then(|id| places.get_key_value(id)) {
                Some(place) => Some(place),
                None => return Err(refused()),
            },
            None => match places.len() {
                0 => None,
                1 => places.iter().next(),
                _ => {
                    let at: Vec<String> = places.keys().map(|id| id.to_string()).collect();
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
        let already = format!("{} and {} are already tangent", self.noun(a), self.noun(b));
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
            Some((at, Place::Held { curve, on, ons })) => {
                let at = Reference::Element((*at).clone());
                (
                    Form::TangentOn {
                        direction: sketch_edit::drawn(self.read, self.read, curve, &at, on),
                        a: (*curve).clone(),
                        at,
                        b: (*on).clone(),
                    },
                    ons.clone(),
                )
            }
        };
        Ok(Plan::Constraints {
            forms: vec![form],
            replaces,
            already,
        })
    }

    /// The places `a` and `b` can be tangent at, by point (C3): each end they share, then
    /// each end of one that an `on` or a `tangent_on` holds on the other.
    fn places(&self, a: &'p Reference, b: &'p Reference) -> BTreeMap<&'p ElementId, Place<'p>> {
        let (ends_a, ends_b) = (ends(self.read, a), ends(self.read, b));
        let mut places = BTreeMap::new();
        for end in &ends_a {
            if ends_b.contains(end) {
                places.entry(*end).or_insert(Place::Shared);
            }
        }
        for (curve, on, curve_ends) in [(a, b, &ends_a), (b, a, &ends_b)] {
            for end in curve_ends {
                let end_ref = Reference::Element((*end).clone());
                let held = ons(self.read, &end_ref, on);
                if !held.is_empty() || tangent_on_holds(self.read, &end_ref, on) {
                    places.entry(*end).or_insert(Place::Held {
                        curve,
                        on,
                        ons: held,
                    });
                }
            }
        }
        places
    }

    /// An edge `tangent` of `a` and `b`, its `side` read from the drawing (C3).
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
                let Some((centre, _)) = round_of(round) else {
                    return Err(refused());
                };
                let side = cross(sub(*to, *from), sub(centre, *from));
                if side > 0.0 {
                    TangentSide::Left
                } else if side < 0.0 {
                    TangentSide::Right
                } else {
                    return Err(self.refusal(format!(
                        "the centre of {} is on {}: which side it's tangent on is unclear; move one off the other first",
                        self.noun(round_ref),
                        self.noun(line)
                    )));
                }
            }
            (round_a, round_b) => {
                let (Some((ca, ra)), Some((cb, rb))) = (round_of(round_a), round_of(round_b))
                else {
                    return Err(refused());
                };
                if distance(ca, cb) >= ra.max(rb) {
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

/// Each member of `picks` after the first, paired with the first (C8), `form(first, other)`.
fn star(picks: &[Reference], form: impl Fn(Reference, Reference) -> Form) -> Vec<Form> {
    let Some((first, rest)) = picks.split_first() else {
        return Vec::new();
    };
    rest.iter()
        .map(|other| form(first.clone(), other.clone()))
        .collect()
}

/// The sketch's `on`s of `point` on `curve`, by ID, in the sketch's order.
fn ons(read: &Sketch, point: &Reference, curve: &Reference) -> Vec<ElementId> {
    read.constraints
        .iter()
        .filter(|SketchConstraint { form, .. }| {
            matches!(form, Form::On { point: p, curve: c } if p == point && c == curve)
        })
        .map(|c| c.id.clone())
        .collect()
}

/// Whether a `tangent_on` of the sketch holds `point` on `curve`: its `at` on its `b`, as an
/// `on` holds a point (the gate's tangent rule reads it so, #258).
fn tangent_on_holds(read: &Sketch, point: &Reference, curve: &Reference) -> bool {
    read.constraints.iter().any(|SketchConstraint { form, .. }| {
        matches!(form, Form::TangentOn { at, b, .. } if at == point && b == curve)
    })
}

/// A line's or an arc's two ends; none for anything else.
fn ends<'s>(read: &'s Sketch, curve: &Reference) -> Vec<&'s ElementId> {
    let Some(id) = curve.element() else {
        return Vec::new();
    };
    if let Some(line) = read.lines.iter().find(|l| &l.id == id) {
        return vec![&line.from, &line.to];
    }
    read.arcs
        .iter()
        .find(|a| &a.id == id)
        .map(|arc| vec![&arc.start, &arc.end])
        .unwrap_or_default()
}

/// A round's centre and radius: none for a line.
fn round_of(geo: &Geo) -> Option<(Xy, f64)> {
    match geo {
        Geo::Circle { center, radius } | Geo::Arc { center, radius, .. } => {
            Some((*center, *radius))
        }
        Geo::Line { .. } => None,
    }
}

/// Where a point picked is: the origin's place, or the read's coordinates.
fn place(view: &View<'_>, point: &Reference) -> Option<Xy> {
    match point {
        Reference::Origin => Some([0.0, 0.0]),
        Reference::Element(id) => view.point(id),
        Reference::XAxis | Reference::YAxis => None,
    }
}

/// "a", "a and b", "a, b, and c".
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// "is" for one, "are" for more.
fn be(n: usize) -> &'static str {
    if n == 1 { "is" } else { "are" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_reads_with_and_and_a_serial_comma() {
        let names = |n: usize| (1..=n).map(|i| i.to_string()).collect::<Vec<_>>();
        assert_eq!(listed(&names(0)), "");
        assert_eq!(listed(&names(1)), "1");
        assert_eq!(listed(&names(2)), "1 and 2");
        assert_eq!(listed(&names(3)), "1, 2, and 3");
        assert_eq!(listed(&names(4)), "1, 2, 3, and 4");
    }
}
