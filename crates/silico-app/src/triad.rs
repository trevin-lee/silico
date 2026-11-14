//! The triad and the view label, at the viewport's bottom left: the approved prototype's
//! triad (docs/design/m5-ui/js/viewport.js, `drawOverlay`, "Triad, bottom left") and its view
//! label (styles.css's `.view-label`, placed beside the triad by app.js's `renderCanvasUI` and
//! written again by interact.js's `onView` whenever the view changes), each drawn with egui's
//! painter in screen space, as DESIGN.md's *egui mapping* has everything drawn over the
//! viewport.
//!
//! The triad is the document's X, Y, and Z as the camera sees them, each in its colour
//! ([`theme::AXIS_X`], [`theme::AXIS_Y`], [`theme::AXIS_Z`]): from an origin
//! [`theme::TRIAD_INSET`] in from the canvas's left and up from its bottom, each axis
//! [`theme::TRIAD_LENGTH`] long times its part across the view, with its letter
//! [`theme::TRIAD_LABEL_PAST`] past its end. It turns with the camera, by the camera's own
//! right and up as the ViewCube does, and keeps its size whatever the zoom or the projection,
//! as the prototype's does: it's directions, not a projection of the model. The axes are drawn
//! the farthest from the viewer first, each its stroke and then its letter, so the nearest is
//! on top; then a dot over the origin.
//!
//! The view label is the projection, "Orthographic" or "Perspective", and at a view the
//! prototype names, " · " and its name: "Orthographic · Iso" as a part opens at the home
//! view. A pan or a zoom keeps the name, as they keep the view's direction; an orbit drops it.
//! The views it names are the prototype's (`VP.VIEWS`): the six faces, by the ViewCube's
//! words, and its `iso`, the app's isometric view ([`navigate::ISOMETRIC`], the home view's
//! corner). The ViewCube's other seven corners, which the prototype doesn't name, nor
//! SolidWorks' standard views (*Isometric* alone among the corners), aren't named either.
//! A view is at a named one by its direction alone ([`navigate::at_view`]), as the
//! prototype's `viewName` compares elevation and azimuth alone.
//!
//! Neither takes the pointer (`pointer-events: none`), and neither hides in sketch mode or
//! under a dialog: the prototype draws both whenever the canvas shows. Each is data first
//! ([`triad`], [`view_label`]), turned into egui's shapes by one function of its own
//! ([`Triad::shapes`], [`ViewLabel::shape`]), so it's tested without a GPU. The viewport pane
//! paints the triad over the bodies and under what the tab's mode draws (the prototype's
//! overlay draws it first), and the label over that (the prototype's is HTML over the
//! overlay), the label's words a label in the accessibility tree too.

use eframe::egui::{FontId, Galley, Painter, Pos2, Rect, Shape, Stroke, pos2};
use silico_core::views::NamedView;
use silico_protocol::{Camera, Projection};

use crate::navigate;
use crate::theme;

/// One of the triad's axes, as drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriadAxis {
    /// Its letter: `X`, `Y`, or `Z`.
    pub letter: &'static str,
    pub colour: eframe::egui::Color32,
    /// Its end, in the window's points: the origin, moved [`theme::TRIAD_LENGTH`] times the
    /// axis's part along the view's right, rightward, and along its up, upward (the screen's
    /// y down). At the origin for an axis seen end on.
    pub end: Pos2,
    /// Where its letter stands: [`theme::TRIAD_LABEL_PAST`] past `end`, along the axis as
    /// drawn. The letter's middle across is here, its baseline [`theme::TRIAD_LABEL_DROP`]
    /// below.
    pub letter_at: Pos2,
    /// How far it points toward the viewer: its part along the view's back, from the target
    /// toward the eye, from −1 (straight away) to 1 (straight at the viewer).
    pub toward: f64,
}

/// The triad, as drawn over a canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triad {
    /// Its origin, in the window's points.
    pub origin: Pos2,
    /// Its axes, in the order they're drawn: the one pointing farthest from the viewer first
    /// (the least `toward`), the nearest last, on top; those pointing as far in X, Y, Z
    /// order (the prototype sorts them so, its sort stable).
    pub axes: [TriadAxis; 3],
}

/// The triad over the canvas `rect` (in the window's points), seen by `camera`; none for a
/// camera that makes no frame ([`Camera::frame`]), or a canvas whose corner isn't a finite
/// place.
pub fn triad(camera: &Camera, rect: Rect) -> Option<Triad> {
    let frame = camera.frame().ok()?;
    let origin = pos2(
        rect.left() + theme::TRIAD_INSET,
        rect.bottom() - theme::TRIAD_INSET,
    );
    if !origin.is_finite() {
        return None;
    }
    let length = f64::from(theme::TRIAD_LENGTH);
    let past = f64::from(theme::TRIAD_LENGTH + theme::TRIAD_LABEL_PAST);
    // An axis on the screen, a unit of the document's along it `scale` points long square
    // across the view.
    let across = |k: usize, scale: f64| {
        let mut unit = [0.0; 3];
        unit[k] = 1.0;
        crate::viewport::on_screen(&frame, unit, scale)
    };
    let axis = |k: usize, letter: &'static str, colour: theme::Rgba8| TriadAxis {
        letter,
        colour: theme::color32(colour),
        end: origin + across(k, length),
        letter_at: origin + across(k, past),
        toward: frame.back[k],
    };
    let mut axes = [
        axis(0, "X", theme::AXIS_X),
        axis(1, "Y", theme::AXIS_Y),
        axis(2, "Z", theme::AXIS_Z),
    ];
    // Stable, as the prototype's `Array.prototype.sort`.
    axes.sort_by(|a, b| a.toward.total_cmp(&b.toward));
    Some(Triad { origin, axes })
}

impl Triad {
    /// Its shapes, in the order they're painted: for each axis in its order, its stroke from
    /// the origin to its end, [`theme::TRIAD_WIDTH`] wide in its colour with a round cap at
    /// its end (a disc as wide as the stroke, its radius half the stroke's width; the cap at
    /// the origin is under the dot), then its letter, [`theme::TRIAD_LABEL_SIZE`] high in its colour, its middle
    /// across at `letter_at` and its baseline [`theme::TRIAD_LABEL_DROP`] below; then the dot
    /// over the origin. An axis seen end on has no stroke, its letter at the origin, as the
    /// prototype draws it (a line of no length, its caps under the dot).
    pub fn shapes(&self, painter: &Painter) -> Vec<Shape> {
        let _span = tracing::debug_span!("triad").entered();
        let mut shapes = Vec::new();
        for axis in &self.axes {
            if axis.end != self.origin {
                let stroke = Stroke::new(theme::TRIAD_WIDTH, axis.colour);
                shapes.push(Shape::line_segment([self.origin, axis.end], stroke));
                shapes.push(Shape::circle_filled(
                    axis.end,
                    theme::TRIAD_WIDTH / 2.0,
                    axis.colour,
                ));
            }
            let font = FontId::proportional(theme::TRIAD_LABEL_SIZE);
            let galley = painter.layout_no_wrap(axis.letter.to_owned(), font, axis.colour);
            let corner = pos2(
                axis.letter_at.x - galley.size().x / 2.0,
                axis.letter_at.y + theme::TRIAD_LABEL_DROP - baseline(&galley),
            );
            shapes.push(Shape::galley(corner, galley, axis.colour));
        }
        shapes.push(Shape::circle_filled(
            self.origin,
            theme::TRIAD_ORIGIN_RADIUS,
            theme::TRIAD_ORIGIN_FILL,
        ));
        shapes
    }
}

/// How far below a galley's top its first line's baseline is: its first glyph's (0 for a
/// galley with none).
pub fn baseline(galley: &Galley) -> f32 {
    galley.rows.first().map_or(0.0, |row| {
        row.pos.y + row.row.glyphs.first().map_or(0.0, |glyph| glyph.pos.y)
    })
}

/// The views the label names, in the prototype's order (`VP.VIEWS`), and the word for its
/// isometric view, its `iso` capitalised as it writes it (`vn[0].toUpperCase() +
/// vn.slice(1)`); the faces are the ViewCube's words, "Front" to "Bottom", as the prototype's
/// keys capitalised are.
const LABELLED_VIEWS: [NamedView; 7] = [
    NamedView::Front,
    NamedView::Back,
    NamedView::Left,
    NamedView::Right,
    NamedView::Top,
    NamedView::Bottom,
    navigate::ISOMETRIC,
];
const ISO: &str = "Iso";

/// The name the view label gives the view `camera` is at, if it's at one it names: the first
/// of [`LABELLED_VIEWS`] it's at ([`navigate::at_view`]; it's never within reach of two).
pub fn view_name(camera: &Camera) -> Option<String> {
    let view = LABELLED_VIEWS
        .into_iter()
        .find(|view| navigate::at_view(camera, *view))?;
    Some(if view == navigate::ISOMETRIC {
        ISO.to_owned()
    } else {
        crate::viewport::words(view)
    })
}

/// The view label, as drawn: its words, and its line's bottom left in the window's points.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewLabel {
    pub words: String,
    pub bottom_left: Pos2,
}

/// The view label over the canvas `rect` (in the window's points), seen by `camera`: its
/// words, the projection ("Perspective", or "Orthographic") and, at a view it names
/// ([`view_name`]), " · " and that view's name; its line's bottom left
/// [`theme::VIEW_LABEL_LEFT`] in from the canvas's left and [`theme::VIEW_LABEL_BOTTOM`] up
/// from its bottom.
pub fn view_label(camera: &Camera, rect: Rect) -> ViewLabel {
    let perspective = matches!(camera.projection, Projection::Perspective { .. });
    let projection = crate::words::projection_words(perspective);
    let words = match view_name(camera) {
        Some(name) => format!("{projection} · {name}"),
        None => projection.to_owned(),
    };
    ViewLabel {
        words,
        bottom_left: pos2(
            rect.left() + theme::VIEW_LABEL_LEFT,
            rect.bottom() - theme::VIEW_LABEL_BOTTOM,
        ),
    }
}

impl ViewLabel {
    /// Its shape: its words, [`theme::VIEW_LABEL_SIZE`] high in [`theme::VIEW_LABEL_TEXT`],
    /// laid out on its line from its left, [`theme::VIEW_LABEL_LINE_HEIGHT`] high, centred
    /// down it (as CSS centres a line's text in its line, and the app's menus centre theirs);
    /// and its line's place, as wide as its words (the label's box shrinks to them).
    pub fn shape(&self, painter: &Painter) -> (Shape, Rect) {
        let font = FontId::proportional(theme::VIEW_LABEL_SIZE);
        let galley = painter.layout_no_wrap(self.words.clone(), font, theme::VIEW_LABEL_TEXT);
        let line = Rect::from_min_max(
            pos2(
                self.bottom_left.x,
                self.bottom_left.y - theme::VIEW_LABEL_LINE_HEIGHT,
            ),
            pos2(self.bottom_left.x + galley.size().x, self.bottom_left.y),
        );
        let corner = pos2(
            line.left(),
            line.top() + (theme::VIEW_LABEL_LINE_HEIGHT - galley.size().y) / 2.0,
        );
        (Shape::galley(corner, galley, theme::VIEW_LABEL_TEXT), line)
    }
}

#[cfg(test)]
mod tests;
