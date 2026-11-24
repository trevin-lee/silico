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
//! dashed right to left, a crossing), and its release no longer clicks: it selects what the
//! box takes. A press that never moves that far is the click it was (on empty space, it
//! clears the selection). A Cancel lets the box go and changes nothing.
//!
//! **What it takes** is decided against what the frame before drew ([`Drawn`]), what the
//! person saw when they let go: each member's strokes, points, arrowheads, and labels, in
//! screen points, the box clamped to the canvas that drawing was made on. A member is
//! inside when every point of everything drawn of it is; it crosses when one is inside, or
//! one of its strokes' segments meets the box (`crate::box_select::ScreenBox::takes`, the
//! canvas's rule). What isn't drawn (hidden by the palette's *Show*, or off the part of the
//! plane drawn) is never taken. The members taken are the sketch's points, lines, circles,
//! and arcs, and its dimensions, never a relation, the origin, or an axis: SolidWorks'
//! "sketch entities and dimensions". A dimension's value is taken at its label's anchor,
//! not its text's extent.
//!
//! Let go, the box replaces the selection; with Shift it adds what's inside; with Ctrl it
//! inverts what's inside, selections outside it staying either way; with both, Shift's
//! (`How::of_box`, the canvas's; [`SketchSelection::boxed`]). What joins the selection
//! joins in ID order, after what's already selected (the constraint tools' C9).

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{Painter, Pos2, Rect};
use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind};

use super::draw::Drawn;
use super::hit::{Hit, SketchPick};
use super::style::SKETCH_DRAG_THRESHOLD;
use crate::box_select::{Piece, ScreenBox, draw_rect};

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

    /// What it takes of `drawn`, the drawing of `read`, made on a canvas `clip`.
    pub fn taken(&self, drawn: &[Drawn], read: &SketchRead, clip: Rect) -> BTreeSet<SketchPick> {
        let area = ScreenBox::in_rect(self.from, self.to, clip);
        taken(drawn, read, &area, self.crossing())
    }
}

/// The members of `read` a box `area` takes of `drawn`: wholly inside, or also crossing it
/// if `crossing`. Its points, lines, circles, and arcs, and its dimensions; by ID.
pub(crate) fn taken(
    drawn: &[Drawn],
    read: &SketchRead,
    area: &ScreenBox,
    crossing: bool,
) -> BTreeSet<SketchPick> {
    let kinds: BTreeMap<&ElementId, ElementKind> = read.members.elements().collect();
    let dimensions: BTreeSet<&ElementId> = read
        .members
        .constraints
        .iter()
        .filter(|constraint| constraint.form.is_dimension())
        .map(|constraint| &constraint.id)
        .collect();
    // Each member's pieces; a profile's fill is its region's, never its first curve's own.
    let mut pieces: BTreeMap<&ElementId, Vec<&Drawn>> = BTreeMap::new();
    for d in drawn {
        if let Some(of) = d.of()
            && !matches!(d, Drawn::Fill { .. })
        {
            pieces.entry(of).or_default().push(d);
        }
    }
    pieces
        .into_iter()
        .filter_map(|(id, pieces)| {
            let kind = *kinds.get(id)?;
            let wanted = match kind {
                ElementKind::Point | ElementKind::Line | ElementKind::Circle | ElementKind::Arc => {
                    true
                }
                ElementKind::Constraint => dimensions.contains(id),
            };
            let drawn = pieces.into_iter().map(|piece| Piece {
                points: piece
                    .positions()
                    .into_iter()
                    .map(|p| [f64::from(p.x), f64::from(p.y)])
                    .collect(),
                read: true,
                path: matches!(piece, Drawn::Stroke { .. }),
            });
            (wanted && area.takes(drawn, crossing)).then(|| {
                SketchPick::Member(Hit {
                    id: id.clone(),
                    kind,
                })
            })
        })
        .collect()
}
