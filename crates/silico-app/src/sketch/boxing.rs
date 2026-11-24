//! Box and crossing selection in sketch mode (docs/design/m5.0-app.md, section 9's
//! *Selection*: "a drag from empty space draws a box; left to right takes only what's
//! wholly inside, and right to left also what it crosses; Shift with a box adds, and Ctrl
//! with a box inverts inside it"; SolidWorks' Box Selection and Cross Selection,
//! docs/ux/solidworks.md §1: "in a sketch, a cross selection takes sketch entities and
//! dimensions").
//!
//! With no tool on, a left press on empty space (nothing within the pick tolerance, and no
//! drag begun) may become a box: once the pointer moves farther from the press than a drag
//! of the sketch's geometry must ([`SKETCH_DRAG_THRESHOLD`]), it's a box, drawn as the
//! canvas's box is (`crate::box_select::draw_rect`: solid dragged left to right, a window;
//! dashed right to left, a crossing), and its release selects nothing as a click would. A
//! press that never moves that far is the click it was (on empty space, it clears the
//! selection). A Cancel lets the box go and changes nothing.
//!
//! **What it takes** is decided against what the frame before drew ([`Drawn`]), what the
//! person saw when they let go: each member's strokes, points, arrowheads, and labels, in
//! screen points, the box clamped to the canvas. A member is inside when every point of
//! everything drawn of it is; it crosses when one is inside, or one of its strokes'
//! segments meets the box. What isn't drawn (hidden by the palette's *Show*, or off the
//! part of the plane drawn) is never taken. The members taken are the sketch's points,
//! lines, circles, and arcs, and its dimensions (a constraint drawn with a value), never a
//! relation's glyph, the origin, or an axis: SolidWorks' "sketch entities and
//! dimensions". A dimension's value is taken at its label's anchor, not its text's extent.
//!
//! Let go, the box replaces the selection; with Shift it adds what's inside; with Ctrl it
//! inverts what's inside, selections outside it staying either way
//! ([`SketchSelection::boxed`]). What joins the selection joins in ID order, after what's
//! already selected (the constraint tools' C9).

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{Painter, Pos2, Rect};
use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind};

use super::draw::{Drawn, LabelKind};
use super::hit::{Hit, SketchPick};
use super::style::SKETCH_DRAG_THRESHOLD;
use crate::box_select::{PxBox, draw_rect};
use crate::selection::How;

/// A left press on empty space, which a move past the threshold makes a box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SketchBox {
    /// Where it was pressed, and where the pointer is, in screen points.
    pub from: Pos2,
    pub to: Pos2,
    /// Whether it's a box: the pointer has moved past the threshold from the press.
    pub open: bool,
}

impl SketchBox {
    /// A press at `at`, not yet a box.
    pub fn pressed(at: Pos2) -> Self {
        SketchBox {
            from: at,
            to: at,
            open: false,
        }
    }

    /// The pointer at `at`: a box once it's farther from the press than a drag must be.
    /// Whether it opened now.
    pub fn moved(&mut self, at: Pos2) -> bool {
        self.to = at;
        let opened = !self.open && (at - self.from).length() > SKETCH_DRAG_THRESHOLD;
        self.open |= opened;
        opened
    }

    /// Whether it's a crossing: dragged right to left.
    pub fn crossing(&self) -> bool {
        self.to.x < self.from.x
    }

    /// The box drawn with `painter`, clipped to its clip rectangle, if it's open.
    pub fn draw(&self, painter: &Painter) {
        if self.open {
            draw_rect(painter, self.from, self.to);
        }
    }

    /// What it takes of `drawn`, the drawing of `read`, on a canvas `clip`.
    pub fn taken(&self, drawn: &[Drawn], read: &SketchRead, clip: Rect) -> BTreeSet<SketchPick> {
        let area = PxBox::in_rect(self.from, self.to, clip);
        taken(drawn, read, &area, self.crossing())
    }
}

/// How a box let go with `shift` and `ctrl` held changes the selection: Shift adds, Ctrl
/// inverts inside it (Ctrl first, as the canvas's box takes them), and a plain box replaces.
pub(crate) fn how(shift: bool, ctrl: bool) -> How {
    if ctrl {
        How::Toggle
    } else if shift {
        How::Add
    } else {
        How::Replace
    }
}

/// The members of `read` a box `area` takes of `drawn`: wholly inside, or also crossing it
/// if `crossing`. Its points, lines, circles, and arcs, and its constraints drawn with a
/// value (dimensions); by ID.
pub(crate) fn taken(
    drawn: &[Drawn],
    read: &SketchRead,
    area: &PxBox,
    crossing: bool,
) -> BTreeSet<SketchPick> {
    let kinds: BTreeMap<&ElementId, ElementKind> = read.members.elements().collect();
    // Each member's pieces: its strokes' polylines, and its single places.
    let mut pieces: BTreeMap<&ElementId, Vec<&Drawn>> = BTreeMap::new();
    let mut dimensions: BTreeSet<&ElementId> = BTreeSet::new();
    for d in drawn {
        let Some(of) = d.of() else { continue };
        match d {
            // A profile's fill is its region's, never its first curve's own.
            Drawn::Fill { .. } => continue,
            Drawn::Label {
                kind: LabelKind::Dimension,
                ..
            } => {
                dimensions.insert(of);
            }
            _ => {}
        }
        pieces.entry(of).or_default().push(d);
    }
    pieces
        .into_iter()
        .filter_map(|(id, pieces)| {
            let kind = *kinds.get(id)?;
            let entity = matches!(
                kind,
                ElementKind::Point | ElementKind::Line | ElementKind::Circle | ElementKind::Arc
            );
            let wanted = entity || (kind == ElementKind::Constraint && dimensions.contains(id));
            (wanted && decide(&pieces, area, crossing)).then(|| {
                SketchPick::Member(Hit {
                    id: id.clone(),
                    kind,
                })
            })
        })
        .collect()
}

/// Whether a member drawn as `pieces` is wholly inside `area` (something of it drawn, every
/// point inside), or, for a crossing, meets it (a point inside, or a stroke's segment
/// meeting it).
fn decide(pieces: &[&Drawn], area: &PxBox, crossing: bool) -> bool {
    let point = |p: Pos2| [f64::from(p.x), f64::from(p.y)];
    let mut any = false;
    for piece in pieces {
        let points: Vec<[f64; 2]> = piece.positions().into_iter().map(point).collect();
        if points.is_empty() {
            continue;
        }
        if crossing {
            let meets = match piece {
                Drawn::Stroke { .. } if points.len() > 1 => points
                    .windows(2)
                    .any(|pair| area.meets_segment(pair[0], pair[1])),
                _ => points.iter().any(|p| area.holds(*p)),
            };
            if meets {
                return true;
            }
        } else {
            if !points.iter().all(|p| area.holds(*p)) {
                return false;
            }
            any = true;
        }
    }
    !crossing && any
}
