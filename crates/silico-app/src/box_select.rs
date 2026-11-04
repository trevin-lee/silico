//! Box and crossing selection in the canvas (M5.7; docs/design/m5.0-app.md, section 9's
//! *Selection*: "a drag from empty space draws a box; left to right takes only what's
//! wholly inside, and right to left also what it crosses; in a part, a box takes edges by
//! default; Shift with a box adds, and Ctrl with a box inverts inside it", and "a drag on
//! geometry selects what's under the press, and draws no box"; SolidWorks' Box Selection
//! and Cross Selection, docs/ux/solidworks.md §1).
//!
//! **The press.** A left press on the canvas records what the pointer was over there, as
//! the selection's hover answered for the press's pixel ([`Selection::hovered`]). At the
//! first move farther from the press than egui's click distance (the distance the
//! selection's click is judged by, so a press is either a click or a drag, never both),
//! the drag is decided: on geometry, what's under the press is picked as a click picks it
//! (Ctrl toggling), and no box is drawn; on empty space, a box. If the hover hasn't
//! answered for the press's pixel yet, the decision is held until it has, the pick at the
//! press asked again each frame meanwhile (as the selection's click is), unless a click
//! waiting for its own pick is asking. A held decision is bounded by state, never by a
//! count of frames: it's let go (the drag then does nothing) when the camera moves or the
//! view's bodies change, since the press's pixel no longer shows what it did, and when the
//! button is let go before it was answered. A Cancel ends the press and changes nothing.
//!
//! **The box.** Dragged left to right it's a window, drawn solid: it takes what's wholly
//! inside. Right to left it's a crossing, drawn dashed: it takes what's inside or crosses
//! its outline. The box is clamped to the canvas, so nothing off the canvas is inside it.
//! Let go, it replaces the selection; with Shift it adds ([`How::Add`]); with Ctrl it
//! inverts what's inside ([`How::Toggle`]); through [`Selection::pick_all`], one change.
//!
//! **What's inside** is decided against the displayed mesh, what the person sees: each
//! body drawn (not hidden), its faces' triangles and its edges' polylines as the view's
//! meshes hold them, projected by the renderer's own projection
//! ([`silico_render::render::project`]) and tested in f64. An edge is inside when every one
//! of its points projects inside the box, and crosses when one of its segments meets the
//! box. A face is inside when every vertex of its triangles projects inside, and crosses
//! when one of its triangles overlaps the box. A point that doesn't project (behind the eye
//! in perspective, or nearer than the drawing's cut) is never inside, so an element with
//! one isn't wholly inside; a segment or a triangle with one is tested by its points that
//! project alone. This is the mesh, not the exact B-rep: a curved edge's chord can lie
//! inside a box its true arc bulges out of, and a face's facets likewise; the box takes it,
//! as what's drawn is inside. A face or an edge the mesh has nothing of (a sliver OCCT
//! couldn't mesh) is never taken. What's behind other faces is taken too: the box doesn't
//! test what's seen in front.
//!
//! **What a box takes** (SolidWorks: "in a part, a box selects edges by default; selection
//! filters select other types"): with the selection filter as it starts, every kind on,
//! edges alone; with the filter changed, the kinds it leaves on (faces, edges; vertices
//! once they can be picked, M5.7's part 3).
//!
//! The visuals are behind [`draw_box`], which the UI port may replace.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{self, Painter, Rect, Shape, Stroke};
use silico_core::document::FeatureId;
use silico_core::naming::Name;
use silico_protocol::Camera;
use silico_render::render::project;

use crate::renderer::{Element, PixelRect};
use crate::selection::{EDGE_BAND_POINTS, Filter, How, Selection};
use crate::theme;
use crate::viewport::{LeftEvent, LeftKind, View, ViewBody, ViewFrame};

/// The canvas's box selection: the left press, while one is held, and the view the
/// press's answer is of.
#[derive(Debug, Default)]
pub struct BoxSelect {
    press: Option<Press>,
    /// The camera and the bodies table last seen: once either changes, a held decision is
    /// let go, and answers to frames before the change are passed over.
    seen: Option<(Camera, u64)>,
    stale_to: Option<u64>,
}

/// A left press held on the canvas.
#[derive(Debug, Clone, PartialEq)]
struct Press {
    /// Where it was pressed and where the pointer is now, in physical pixels from the
    /// canvas's top left.
    at_px: [f32; 2],
    corner: [f32; 2],
    /// The pixel it was pressed on, as the hover's pick centres on it.
    pixel: [u32; 2],
    /// What the hover answered for the press's pixel: an element by its name, or nothing;
    /// none until it has answered.
    under: Option<Option<Arc<Name>>>,
    phase: Phase,
    /// The modifiers of its newest event.
    ctrl: bool,
    shift: bool,
}

/// Where a press stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Within the click distance of where it was pressed: a click so far, the selection's.
    Pressed,
    /// Past it, the hover's answer for the press's pixel not come yet.
    Held,
    /// On geometry: what was under it is picked, and no box drawn.
    Geometry,
    /// On empty space: a box.
    Boxing,
    /// Let go undecided: the view changed under it. Nothing more until its end.
    LetGo,
}

/// What a frame's events and answers ask of the selection and the view.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    /// The element under the press picked, as a click picks it.
    Pick(Arc<Name>, How),
    /// A box let go: from one corner to the other, in physical pixels, a crossing or not.
    Box {
        from: [f32; 2],
        to: [f32; 2],
        crossing: bool,
        how: How,
    },
}

/// What a frame tells [`BoxSelect::step`]: the left button's events; the freshest answer
/// for a pixel (the pixel, and what the hover found there), if there is one now; whether
/// the view changed since the last frame; the click distance, in physical pixels; and the
/// canvas's size.
struct Told<'a> {
    left: &'a [LeftEvent],
    heard: Option<([u32; 2], Option<Arc<Name>>)>,
    changed: bool,
    click_px: f32,
    size_px: [u32; 2],
}

impl BoxSelect {
    /// The canvas's left button this frame (`frame`, given to the tab's overlay after the
    /// selection has taken its clicks and hover): a drag on geometry picked, a box drawn on
    /// `painter` and, let go, selected; the pick at the press asked again of `view` while
    /// its answer is awaited. `current` is whether the view's bodies are of the model the
    /// selection is read against (nothing is decided or selected from other bodies);
    /// `shown`, whether a body is drawn (the browser hides some).
    pub(crate) fn overlay(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        frame: &ViewFrame,
        painter: &Painter,
        current: bool,
        shown: &dyn Fn(&FeatureId) -> bool,
    ) {
        let ppp = frame.pixels_per_point;
        let click_px = painter
            .ctx()
            .options(|options| options.input_options.max_click_dist)
            * ppp;
        let changed = self.view_changed(view, frame);
        let heard = self.heard(view, current).map(|pixel| {
            let under = selection.hovered().map(|hovered| Arc::clone(&hovered.name));
            (pixel, under)
        });
        let effects = self.step(&Told {
            left: &frame.left,
            heard,
            changed,
            click_px,
            size_px: frame.size_px,
        });
        for effect in effects {
            match effect {
                Effect::Pick(name, how) => selection.pick(&name.to_string(), how),
                Effect::Box {
                    from,
                    to,
                    crossing,
                    how,
                } if current => {
                    let area = PxBox::clamped(from, to, frame.size_px);
                    let names = taken(
                        view.bodies(),
                        shown,
                        kinds(selection.filter),
                        &frame.camera,
                        frame.size_px,
                        area,
                        crossing,
                    );
                    tracing::debug!(crossing, ?how, taken = names.len(), "box_select");
                    selection.pick_all(&names, how);
                }
                // Let go over bodies of another model than the selection's: what's drawn
                // isn't what its names would be read against.
                Effect::Box { .. } => {
                    tracing::debug!("a box let go over meshes of another model: nothing taken");
                }
            }
        }
        if current {
            self.ask(view, frame);
        }
        if let Some(press) = &self.press
            && press.phase == Phase::Boxing
        {
            draw_box(painter, frame, press.at_px, press.corner);
        }
    }

    /// Whether the camera or the bodies table changed since the last frame: if so, the
    /// newest frame that carried a pick before it is noted, so answers to it or an earlier
    /// one are passed over, as the selection's hover passes them over.
    fn view_changed(&mut self, view: &View, frame: &ViewFrame) -> bool {
        let now = (frame.camera.clone(), view.table());
        let changed = self.seen.as_ref().is_some_and(|seen| *seen != now);
        if changed {
            self.stale_to = view.last_asked();
        }
        self.seen = Some(now);
        changed
    }

    /// The pixel whose pick the newest answer is to, if one is in and the hover took it:
    /// the view's bodies `current`, and the answer to a frame since the view last changed.
    /// The selection's hover then holds what it found there.
    fn heard(&self, view: &View, current: bool) -> Option<[u32; 2]> {
        if !current {
            return None;
        }
        let picking = view.picked()?;
        if self.stale_to.is_some_and(|stale| picking.frame <= stale) {
            return None;
        }
        Some(centre_of(picking.rect))
    }

    /// The press's pick asked again, while its answer is awaited, unless a click waiting
    /// for its own pick asked this frame: what the selection asked is replaced only if it's
    /// the hover's (its square about the pointer's pixel), or nothing was.
    fn ask(&mut self, view: &mut View, frame: &ViewFrame) {
        let Some(press) = &self.press else {
            return;
        };
        if press.under.is_some() || !matches!(press.phase, Phase::Pressed | Phase::Held) {
            return;
        }
        let band = band_px(frame.pixels_per_point);
        let hover = frame
            .pointer_px
            .map(|pointer| pixel_of(pointer, frame.size_px));
        let ours = square(press.pixel, band);
        match view.asking() {
            Some(asked) if asked == ours => {}
            Some(asked) if Some(centre_of(asked)) != hover => {}
            _ => {
                view.pick(ours);
            }
        }
    }

    /// The frame `told` taken: each left event in turn, the press's answer learnt once it's
    /// heard, and what that decides.
    fn step(&mut self, told: &Told<'_>) -> Vec<Effect> {
        let mut effects = Vec::new();
        if told.changed
            && let Some(press) = &mut self.press
            && matches!(press.phase, Phase::Pressed | Phase::Held)
        {
            tracing::debug!(?press.phase, "the view changed under a press undecided: let go");
            press.phase = Phase::LetGo;
        }
        for event in told.left {
            match event.kind {
                LeftKind::Press => {
                    self.press = Some(Press {
                        at_px: event.at_px,
                        corner: event.at_px,
                        pixel: pixel_of(event.at_px, told.size_px),
                        under: None,
                        phase: Phase::Pressed,
                        ctrl: event.ctrl,
                        shift: event.shift,
                    });
                    self.learn(told, &mut effects);
                }
                LeftKind::Move => {
                    if let Some(press) = &mut self.press {
                        press.corner = event.at_px;
                        press.ctrl = event.ctrl;
                        press.shift = event.shift;
                        if press.phase == Phase::Pressed
                            && distance(press.at_px, event.at_px) > told.click_px
                        {
                            press.phase = Phase::Held;
                        }
                    }
                    self.learn(told, &mut effects);
                }
                LeftKind::Release => {
                    let Some(mut press) = self.press.take() else {
                        continue;
                    };
                    press.corner = event.at_px;
                    press.ctrl = event.ctrl;
                    press.shift = event.shift;
                    // Let go past the click distance with no move there between: decided
                    // now, as a move there would have.
                    if press.phase == Phase::Pressed
                        && distance(press.at_px, event.at_px) > told.click_px
                    {
                        press.phase = Phase::Held;
                    }
                    self.press = Some(press);
                    self.learn(told, &mut effects);
                    let Some(press) = self.press.take() else {
                        continue;
                    };
                    match press.phase {
                        Phase::Boxing => {
                            let how = if press.shift {
                                How::Add
                            } else if press.ctrl {
                                How::Toggle
                            } else {
                                How::Replace
                            };
                            effects.push(Effect::Box {
                                from: press.at_px,
                                to: press.corner,
                                crossing: press.corner[0] < press.at_px[0],
                                how,
                            });
                        }
                        Phase::Held => {
                            tracing::debug!("a drag let go before its press was answered");
                        }
                        Phase::Pressed | Phase::Geometry | Phase::LetGo => {}
                    }
                }
                LeftKind::Cancel => self.press = None,
                LeftKind::DoubleClick => {}
            }
        }
        self.learn(told, &mut effects);
        effects
    }

    /// The press's answer taken if it's heard now (the newest answer is to its pixel), and,
    /// if the press is past the click distance, the drag decided by it: on geometry, its
    /// element picked as a click picks it (Ctrl toggling); on empty space, a box.
    fn learn(&mut self, told: &Told<'_>, effects: &mut Vec<Effect>) {
        let Some(press) = &mut self.press else {
            return;
        };
        if press.under.is_none()
            && let Some((pixel, under)) = &told.heard
            && *pixel == press.pixel
            && matches!(press.phase, Phase::Pressed | Phase::Held)
        {
            press.under = Some(under.clone());
        }
        if press.phase != Phase::Held {
            return;
        }
        match &press.under {
            None => {}
            Some(Some(name)) => {
                let how = if press.ctrl {
                    How::Toggle
                } else {
                    How::Replace
                };
                effects.push(Effect::Pick(Arc::clone(name), how));
                press.phase = Phase::Geometry;
            }
            Some(None) => press.phase = Phase::Boxing,
        }
    }
}

/// The kinds a box takes under `filter`: edges alone with the filter as it starts (every
/// kind on: "in a part, a box selects edges by default"); else those it leaves on.
/// Vertices aren't in the displayed meshes yet (M5.7's part 3), so none is taken.
fn kinds(filter: Filter) -> Kinds {
    if filter == Filter::default() {
        Kinds {
            faces: false,
            edges: true,
        }
    } else {
        Kinds {
            faces: filter.faces,
            edges: filter.edges,
        }
    }
}

/// Which kinds of element a box takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Kinds {
    faces: bool,
    edges: bool,
}

/// A box on the canvas, in physical pixels from its top left: `[x0, x1] × [y0, y1]`, its
/// sides counted inside.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PxBox {
    min: [f64; 2],
    max: [f64; 2],
}

impl PxBox {
    /// The box with corners `a` and `b`, clamped to a canvas `size` pixels (what's off the
    /// canvas isn't seen, so it's never inside). A corner that isn't finite is taken as the
    /// other's.
    fn clamped(a: [f32; 2], b: [f32; 2], size: [u32; 2]) -> PxBox {
        let side = |k: usize| {
            let limit = f64::from(size[k]);
            let first = f64::from(a[k]);
            let second = f64::from(b[k]);
            let (first, second) = match (first.is_finite(), second.is_finite()) {
                (true, true) => (first, second),
                (true, false) => (first, first),
                (false, true) => (second, second),
                (false, false) => (0.0, 0.0),
            };
            let low = first.min(second).clamp(0.0, limit);
            let high = first.max(second).clamp(0.0, limit);
            (low, high)
        };
        let (x0, x1) = side(0);
        let (y0, y1) = side(1);
        PxBox {
            min: [x0, y0],
            max: [x1, y1],
        }
    }

    fn holds(&self, p: [f64; 2]) -> bool {
        self.min[0] <= p[0] && p[0] <= self.max[0] && self.min[1] <= p[1] && p[1] <= self.max[1]
    }

    fn corners(&self) -> [[f64; 2]; 4] {
        [
            self.min,
            [self.max[0], self.min[1]],
            self.max,
            [self.min[0], self.max[1]],
        ]
    }

    /// Whether the box and the box spanned by `points` share a point.
    fn overlaps_bounds(&self, points: &[[f64; 2]]) -> bool {
        (0..2).all(|k| {
            let low = points.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min);
            let high = points
                .iter()
                .map(|p| p[k])
                .fold(f64::NEG_INFINITY, f64::max);
            low <= self.max[k] && self.min[k] <= high
        })
    }

    /// Whether the segment from `a` to `b` meets the box (its outline or inside): their
    /// boxes overlap, and the box's corners aren't all strictly on one side of its line
    /// (the separating axes of a segment and a box).
    fn meets_segment(&self, a: [f64; 2], b: [f64; 2]) -> bool {
        if self.holds(a) || self.holds(b) {
            return true;
        }
        if !self.overlaps_bounds(&[a, b]) {
            return false;
        }
        let sides = self.corners().map(|c| orient(a, b, c));
        !(sides.iter().all(|s| *s > 0.0) || sides.iter().all(|s| *s < 0.0))
    }

    /// Whether the triangle `t` overlaps the box (shares a point with it): their boxes
    /// overlap, and no side of the triangle has every corner of the box strictly beyond it
    /// (the separating axes of two convex shapes). A triangle with no area is its sides.
    fn overlaps_triangle(&self, t: [[f64; 2]; 3]) -> bool {
        if !self.overlaps_bounds(&t) {
            return false;
        }
        let area = orient(t[0], t[1], t[2]);
        if area == 0.0 || !area.is_finite() {
            return (0..3).any(|k| self.meets_segment(t[k], t[(k + 1) % 3]));
        }
        (0..3).all(|k| {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            // The third vertex's side of this side's line is the inside.
            !self.corners().iter().all(|c| orient(a, b, *c) * area < 0.0)
        })
    }
}

/// Twice the signed area of `a`, `b`, `c`: positive if they turn one way, negative the
/// other, 0 in a line.
fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// The names of what a box `area` takes of `bodies`, those `shown` alone, of the `kinds`
/// it takes, seen through `camera` on a canvas `size` pixels: wholly inside, or also
/// crossing it if `crossing`. By body in its number's order, then faces, then edges, each
/// by its number: as the element map names them.
fn taken(
    bodies: &BTreeMap<u32, ViewBody>,
    shown: &dyn Fn(&FeatureId) -> bool,
    kinds: Kinds,
    camera: &Camera,
    size: [u32; 2],
    area: PxBox,
    crossing: bool,
) -> Vec<String> {
    let mut names = Vec::new();
    for body in bodies.values().filter(|body| shown(&body.feature)) {
        let mesh = &body.mesh;
        if kinds.faces {
            let projected: Vec<Option<[f64; 2]>> = mesh
                .positions
                .iter()
                .map(|p| project(camera, size, *p))
                .collect();
            let vertex = |index: u32| -> Option<[f64; 2]> {
                usize::try_from(index)
                    .ok()
                    .and_then(|at| projected.get(at).copied().flatten())
            };
            let mut first = 0usize;
            for (count, number) in mesh.face_triangles.iter().zip(1u32..) {
                let count = usize::try_from(*count).unwrap_or(usize::MAX);
                let end = first.saturating_add(count);
                let triangles = mesh.triangles.get(first..end);
                first = end;
                let Some(triangles) = triangles.filter(|t| !t.is_empty()) else {
                    continue;
                };
                let corners = triangles.iter().map(|t| t.map(vertex));
                let take = if crossing {
                    corners.into_iter().any(|t| match t {
                        [Some(a), Some(b), Some(c)] => area.overlaps_triangle([a, b, c]),
                        partial => partial.iter().flatten().any(|p| area.holds(*p)),
                    })
                } else {
                    corners
                        .into_iter()
                        .all(|t| t.iter().all(|p| p.is_some_and(|p| area.holds(p))))
                };
                if take && let Some(named) = body.name_of(Element::Face(number)) {
                    names.push(named.name.to_string());
                }
            }
        }
        if kinds.edges {
            let mut first = 0usize;
            for (count, number) in mesh.edge_point_counts.iter().zip(1u32..) {
                let count = usize::try_from(*count).unwrap_or(usize::MAX);
                let end = first.saturating_add(count);
                let points = mesh.edge_points.get(first..end);
                first = end;
                let Some(points) = points.filter(|p| !p.is_empty()) else {
                    continue;
                };
                let projected: Vec<Option<[f64; 2]>> =
                    points.iter().map(|p| project(camera, size, *p)).collect();
                let take = if crossing {
                    projected.iter().flatten().any(|p| area.holds(*p))
                        || projected.windows(2).any(|pair| match pair {
                            [Some(a), Some(b)] => area.meets_segment(*a, *b),
                            _ => false,
                        })
                } else {
                    projected.iter().all(|p| p.is_some_and(|p| area.holds(p)))
                };
                if take && let Some(named) = body.name_of(Element::Edge(number)) {
                    names.push(named.name.to_string());
                }
            }
        }
    }
    names
}

/// The box from `from` to `to` (physical pixels from the canvas's top left) drawn on
/// `painter` over `frame`'s canvas: [`theme::BOX_SELECT_FILL`], outlined in
/// [`theme::BOX_SELECT_STROKE`], solid for a window (dragged left to right) and dashed for a
/// crossing (right to left). The painter clips it to the canvas.
pub(crate) fn draw_box(painter: &Painter, frame: &ViewFrame, from: [f32; 2], to: [f32; 2]) {
    let ppp = frame.pixels_per_point;
    if !(ppp.is_finite() && ppp > 0.0) {
        return;
    }
    let at = |p: [f32; 2]| frame.rect.min + egui::vec2(p[0] / ppp, p[1] / ppp);
    let (a, b) = (at(from), at(to));
    if !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite()) {
        return;
    }
    let rect = Rect::from_two_pos(a, b);
    let stroke = Stroke::new(
        theme::BOX_SELECT_STROKE_WIDTH,
        theme::color32(theme::BOX_SELECT_STROKE),
    );
    painter.rect_filled(rect, 0.0, theme::color32(theme::BOX_SELECT_FILL));
    let outline = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    if to[0] < from[0] {
        let [dash, gap] = theme::BOX_SELECT_CROSSING_DASH;
        painter.extend(Shape::dashed_line(&outline, stroke, dash, gap));
    } else {
        painter.add(Shape::line(outline.to_vec(), stroke));
    }
}

/// The edge band, in physical pixels, at `ppp` pixels a point, as the selection's pick
/// takes it: at least one.
fn band_px(ppp: f32) -> u32 {
    (EDGE_BAND_POINTS * ppp).ceil().clamp(1.0, 96.0) as u32
}

/// The pixel `at_px` (physical pixels from the canvas's top left, maybe outside it) is in,
/// clamped into a canvas `size` pixels, as the selection's pick centres its square on it.
fn pixel_of(at_px: [f32; 2], size: [u32; 2]) -> [u32; 2] {
    let pixel = |at: f32, side: u32| {
        let last = side.saturating_sub(1);
        // Floored and clamped into [0, last]: a NaN is 0, as `as` takes it.
        (at.floor().max(0.0) as u32).min(last)
    };
    [pixel(at_px[0], size[0]), pixel(at_px[1], size[1])]
}

/// The square `band` pixels out on each side of `pixel`, as the selection's pick asks it.
fn square(pixel: [u32; 2], band: u32) -> PixelRect {
    // A canvas's side is a few thousand pixels: inside an i32.
    let corner = |c: u32| i32::try_from(c).unwrap_or(i32::MAX) - band as i32;
    PixelRect {
        min: [corner(pixel[0]), corner(pixel[1])],
        size: [2 * band + 1, 2 * band + 1],
    }
}

/// A pick's square's centre pixel (its sides are odd).
fn centre_of(rect: PixelRect) -> [u32; 2] {
    let at = |min: i32, side: u32| u32::try_from(i64::from(min) + i64::from(side / 2)).unwrap_or(0);
    [at(rect.min[0], rect.size[0]), at(rect.min[1], rect.size[1])]
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_core::command::meshes::MeshElement;
    use silico_core::naming::{PrimitiveKind, TypeWord};
    use silico_core::recompute::Key;
    use silico_protocol::{ElementKind, ElementRef, Mesh, Projection};

    const BODY: &str = "k7f2qx3m";

    /// From −y onto the origin, z up, orthographic, 128 mm high, on a canvas 256 by 128
    /// pixels: 1 pixel a millimetre, x right from the canvas's middle (128) and z up from it
    /// (64), so `(x, y, z)` lands at `(128 + x, 64 − z)` whatever its y. Its scales are
    /// powers of two, so a pixel given in quarters lands exactly there, in f32 and f64.
    fn front() -> Camera {
        Camera {
            projection: Projection::Orthographic { view_height: 128.0 },
            eye: [0.0, -100.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    const SIZE: [u32; 2] = [256, 128];

    /// The document's point that [`front`] lands on pixel `(px, py)`, at depth `y`.
    fn at(px: f64, py: f64, y: f64) -> [f64; 3] {
        [px - 128.0, y, 64.0 - py]
    }

    fn named(kind: ElementKind, index: u32) -> MeshElement {
        MeshElement {
            name: Arc::new(Name::Element {
                feature: FeatureId::new(BODY).unwrap(),
                made: TypeWord::Primitive(PrimitiveKind::Box),
                element: ElementRef { kind, index },
            }),
            stable: true,
            count: None,
        }
    }

    /// A body of `edges` (polylines, by their pixels on [`front`]) and `faces` (each a list
    /// of triangles, by their corners' pixels), each named `k7f2qx3m.box.Edge<n>` and
    /// `Face<n>` in order.
    fn body(edges: &[&[[f64; 2]]], faces: &[&[[[f64; 2]; 3]]]) -> ViewBody {
        let mut mesh = Mesh {
            positions: Vec::new(),
            normals: Vec::new(),
            triangles: Vec::new(),
            face_triangles: Vec::new(),
            valid: None,
            edge_points: Vec::new(),
            edge_point_counts: Vec::new(),
        };
        for edge in edges {
            mesh.edge_points
                .extend(edge.iter().map(|[x, y]| at(*x, *y, 0.0)));
            mesh.edge_point_counts.push(edge.len() as u32);
        }
        for face in faces {
            for triangle in *face {
                let base = mesh.positions.len() as u32;
                mesh.positions
                    .extend(triangle.iter().map(|[x, y]| at(*x, *y, 5.0)));
                mesh.normals.extend([[0.0, -1.0, 0.0]; 3]);
                mesh.triangles.push([base, base + 1, base + 2]);
            }
            mesh.face_triangles.push(face.len() as u32);
        }
        ViewBody {
            feature: FeatureId::new(BODY).unwrap(),
            key: Key::parse(&"01".repeat(32)).unwrap(),
            level: 0.01,
            deflection: 0.01,
            bounds: None,
            serves: [0.0, 1.0],
            held: false,
            faces: (1..=faces.len() as u32)
                .map(|n| named(ElementKind::Face, n))
                .collect(),
            edges: (1..=edges.len() as u32)
                .map(|n| named(ElementKind::Edge, n))
                .collect(),
            mesh: Arc::new(mesh),
        }
    }

    fn bodies(body: ViewBody) -> BTreeMap<u32, ViewBody> {
        BTreeMap::from([(1, body)])
    }

    const EDGES: Kinds = Kinds {
        faces: false,
        edges: true,
    };
    const FACES: Kinds = Kinds {
        faces: true,
        edges: false,
    };

    fn every(_: &FeatureId) -> bool {
        true
    }

    /// What a box from `a` to `b` (pixels) takes of `bodies` on [`front`], window or
    /// crossing as dragged (right to left a crossing), as short names (`Edge1`).
    fn boxed(
        bodies: &BTreeMap<u32, ViewBody>,
        kinds: Kinds,
        a: [f32; 2],
        b: [f32; 2],
    ) -> Vec<String> {
        let area = PxBox::clamped(a, b, SIZE);
        taken(bodies, &every, kinds, &front(), SIZE, area, b[0] < a[0])
            .into_iter()
            .map(|name| name.trim_start_matches("k7f2qx3m.box.").to_owned())
            .collect()
    }

    #[test]
    fn the_front_camera_lands_points_where_the_tests_say() {
        // The tests' pixels by hand rest on this: (128 + x, 64 − z), exactly.
        for (point, pixel) in [
            ([10.0, 0.0, 5.0], [138.0, 59.0]),
            ([-128.0, 30.0, 64.0], [0.0, 0.0]),
            ([25.5, -3.0, -12.25], [153.5, 76.25]),
            ([-18.0, 0.0, 39.0], [110.0, 25.0]),
        ] {
            assert_eq!(project(&front(), SIZE, point), Some(pixel), "{point:?}");
        }
    }

    #[test]
    fn a_window_takes_the_edge_wholly_inside_and_a_crossing_also_the_one_it_touches() {
        // Edge1 from (110, 40) to (130, 40), wholly in the box (105, 25)–(140, 45); Edge2
        // from (125, 30) to (160, 30), half out; Edge3 from (100, 90) to (120, 90), below;
        // Edge4 from (100, 35) to (150, 35), through the box with no point in it.
        let edges: [&[[f64; 2]]; 4] = [
            &[[110.0, 40.0], [130.0, 40.0]],
            &[[125.0, 30.0], [160.0, 30.0]],
            &[[100.0, 90.0], [120.0, 90.0]],
            &[[100.0, 35.0], [150.0, 35.0]],
        ];
        let held = bodies(body(&edges, &[]));
        // Left to right: a window.
        assert_eq!(boxed(&held, EDGES, [105.0, 25.0], [140.0, 45.0]), ["Edge1"]);
        // The same box right to left: a crossing, also Edge2, which is half in, and Edge4,
        // which runs through it (none of its points is inside).
        assert_eq!(
            boxed(&held, EDGES, [140.0, 25.0], [105.0, 45.0]),
            ["Edge1", "Edge2", "Edge4"]
        );
        // Up or down doesn't matter, only left or right.
        assert_eq!(boxed(&held, EDGES, [105.0, 45.0], [140.0, 25.0]), ["Edge1"]);
    }

    #[test]
    fn an_edge_on_the_boxs_outline_is_inside_and_one_a_hair_out_isnt() {
        // The box's sides count as inside: Edge1 along its top, (110, 25)–(130, 25); Edge2
        // a thousandth of a pixel above it.
        let edges: [&[[f64; 2]]; 2] = [
            &[[110.0, 25.0], [130.0, 25.0]],
            &[[110.0, 24.999], [130.0, 24.999]],
        ];
        let held = bodies(body(&edges, &[]));
        assert_eq!(boxed(&held, EDGES, [105.0, 25.0], [140.0, 45.0]), ["Edge1"]);
        // A crossing doesn't reach it either: it's outside, if barely.
        assert_eq!(boxed(&held, EDGES, [140.0, 25.0], [105.0, 45.0]), ["Edge1"]);
    }

    #[test]
    fn a_window_tests_the_displayed_polyline_not_the_exact_arc_near_its_side() {
        // An arc of radius 10 about pixel (100, 50), from −30° to 30°, displayed as its
        // chord: (100 + 10 cos 30°, 50 ∓ 5), x = 100 + 5√3 = 108.660…. The arc bulges to x = 110
        // at 0°. A window to x = 109.5 holds the chord, though the arc runs 0.5 past it:
        // what's drawn is inside, so the box takes it (a test of the exact B-rep wouldn't).
        // To 108.5 it holds neither.
        let x = 100.0 + 5.0 * 3f64.sqrt();
        let edges: [&[[f64; 2]]; 1] = [&[[x, 45.0], [x, 55.0]]];
        let held = bodies(body(&edges, &[]));
        assert_eq!(boxed(&held, EDGES, [100.0, 40.0], [109.5, 60.0]), ["Edge1"]);
        assert!(boxed(&held, EDGES, [100.0, 40.0], [108.5, 60.0]).is_empty());
    }

    #[test]
    fn a_faces_triangles_decide_it_a_crossing_takes_one_the_box_lies_wholly_inside() {
        // Face1: the square (110, 20)–(150, 40) as two triangles. Face2: a triangle
        // (160, 10), (190, 10), (160, 40).
        let square: &[[[f64; 2]; 3]] = &[
            [[110.0, 20.0], [150.0, 20.0], [150.0, 40.0]],
            [[110.0, 20.0], [150.0, 40.0], [110.0, 40.0]],
        ];
        let corner: &[[[f64; 2]; 3]] = &[[[160.0, 10.0], [190.0, 10.0], [160.0, 40.0]]];
        let held = bodies(body(&[], &[square, corner]));
        // A window round the square alone, then round both.
        assert_eq!(boxed(&held, FACES, [105.0, 15.0], [155.0, 45.0]), ["Face1"]);
        assert_eq!(
            boxed(&held, FACES, [105.0, 5.0], [195.0, 45.0]),
            ["Face1", "Face2"]
        );
        // A crossing inside the square, no vertex of it inside the box: the box overlaps
        // its triangles, so it's taken.
        assert_eq!(boxed(&held, FACES, [140.0, 25.0], [120.0, 35.0]), ["Face1"]);
        // A crossing in the corner's empty half, beyond its long side from (190, 10) to
        // (160, 40) (x + y = 200): (185, 30)–(195, 38) has every corner past it. Taken by
        // its box's overlap alone, it would be.
        assert!(boxed(&held, FACES, [195.0, 30.0], [185.0, 38.0]).is_empty());
        // Moved to touch the long side at (180, 20): taken.
        assert_eq!(boxed(&held, FACES, [190.0, 20.0], [180.0, 30.0]), ["Face2"]);
    }

    #[test]
    fn what_a_box_takes_follows_the_filter_edges_alone_at_first() {
        let edges: [&[[f64; 2]]; 1] = [&[[110.0, 30.0], [130.0, 30.0]]];
        let square: &[[[f64; 2]; 3]] = &[[[110.0, 20.0], [150.0, 20.0], [150.0, 40.0]]];
        let held = bodies(body(&edges, &[square]));
        let everything = |filter: Filter| boxed(&held, kinds(filter), [100.0, 10.0], [160.0, 50.0]);
        let filter = |faces, edges, vertices| Filter {
            faces,
            edges,
            vertices,
        };
        // Every kind on, as the filter starts: edges alone ("in a part, a box takes edges
        // by default").
        assert_eq!(everything(Filter::default()), ["Edge1"]);
        // Edges off (E): faces.
        assert_eq!(everything(filter(true, false, true)), ["Face1"]);
        // Faces off (X): edges.
        assert_eq!(everything(filter(false, true, true)), ["Edge1"]);
        // Vertices off (V): the filter changed, so the kinds it leaves on, faces first.
        assert_eq!(everything(filter(true, true, false)), ["Face1", "Edge1"]);
        // Vertices alone: none drawn yet, so nothing.
        assert!(everything(filter(false, false, true)).is_empty());
    }

    #[test]
    fn a_hidden_body_and_an_unmeshed_face_or_edge_are_never_taken() {
        let square: &[[[f64; 2]; 3]] = &[[[110.0, 20.0], [150.0, 20.0], [150.0, 40.0]]];
        let edges: [&[[f64; 2]]; 2] = [&[], &[[110.0, 30.0], [130.0, 30.0]]];
        let held = bodies(body(&edges, &[&[], square]));
        // Edge1 and Face1 have nothing in the mesh: neither is taken, by a window or a
        // crossing; Edge2 and Face2 are.
        assert_eq!(boxed(&held, EDGES, [100.0, 10.0], [160.0, 50.0]), ["Edge2"]);
        assert_eq!(boxed(&held, FACES, [160.0, 10.0], [100.0, 50.0]), ["Face2"]);
        let area = PxBox::clamped([100.0, 10.0], [160.0, 50.0], SIZE);
        let hidden = |_: &FeatureId| false;
        assert!(taken(&held, &hidden, EDGES, &front(), SIZE, area, false).is_empty());
    }

    #[test]
    fn a_point_behind_the_eye_is_never_inside() {
        // In perspective from (0, −100, 0), a point at y −200 is behind the eye: the edge
        // from it to the origin isn't wholly inside any box, and a crossing takes it only
        // by its point that projects, the origin at the canvas's middle (128, 64).
        let camera = Camera {
            projection: Projection::Perspective {
                field_of_view: 45.0,
            },
            eye: [0.0, -100.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        assert_eq!(project(&camera, SIZE, [0.0, -200.0, 0.0]), None);
        let mut held = body(&[], &[]);
        held.mesh = Arc::new(Mesh {
            edge_points: vec![[0.0, -200.0, 0.0], [0.0, 0.0, 0.0]],
            edge_point_counts: vec![2],
            ..(*held.mesh).clone()
        });
        held.edges = vec![named(ElementKind::Edge, 1)];
        let held = bodies(held);
        let whole = PxBox::clamped([0.0, 0.0], [256.0, 128.0], SIZE);
        assert!(taken(&held, &every, EDGES, &camera, SIZE, whole, false).is_empty());
        assert_eq!(
            taken(&held, &every, EDGES, &camera, SIZE, whole, true),
            ["k7f2qx3m.box.Edge1"]
        );
        let aside = PxBox::clamped([0.0, 0.0], [100.0, 128.0], SIZE);
        assert!(taken(&held, &every, EDGES, &camera, SIZE, aside, true).is_empty());
    }

    #[test]
    fn a_box_is_clamped_to_the_canvas_and_a_corner_that_isnt_finite_is_the_others() {
        assert_eq!(
            PxBox::clamped([150.0, 50.0], [500.0, -20.0], SIZE),
            PxBox {
                min: [150.0, 0.0],
                max: [256.0, 50.0]
            }
        );
        assert_eq!(
            PxBox::clamped([150.0, 50.0], [f32::NAN, f32::INFINITY], SIZE),
            PxBox {
                min: [150.0, 50.0],
                max: [150.0, 50.0]
            }
        );
        // Wholly off the canvas: a line along its side, which holds only what's on it.
        assert_eq!(
            PxBox::clamped([-50.0, 10.0], [-10.0, 20.0], SIZE),
            PxBox {
                min: [0.0, 10.0],
                max: [0.0, 20.0]
            }
        );
        // An edge off the canvas, past its right side, isn't inside a box dragged past it.
        let edges: [&[[f64; 2]]; 2] = [
            &[[246.0, 30.0], [266.0, 30.0]],
            &[[150.0, 30.0], [255.0, 30.0]],
        ];
        let held = bodies(body(&edges, &[]));
        assert_eq!(boxed(&held, EDGES, [140.0, 10.0], [400.0, 50.0]), ["Edge2"]);
    }

    // ---- The press, decided ----

    /// Box `k7f2qx3m`'s element `kind` `index` by its name (`k7f2qx3m.box.Face3`).
    fn name(kind: ElementKind, index: u32) -> Arc<Name> {
        Arc::clone(&named(kind, index).name)
    }

    fn event(kind: LeftKind, at_px: [f32; 2]) -> LeftEvent {
        LeftEvent {
            kind,
            at_px,
            shift: false,
            ctrl: false,
            frame: 0,
            time: 0.0,
            seq: 0,
        }
    }

    fn with(mut event: LeftEvent, shift: bool, ctrl: bool) -> LeftEvent {
        event.shift = shift;
        event.ctrl = ctrl;
        event
    }

    /// One frame of `left`, the answer for `heard` in, at 6 pixels' click distance.
    fn frame(
        boxes: &mut BoxSelect,
        left: &[LeftEvent],
        heard: Option<([u32; 2], Option<Arc<Name>>)>,
        changed: bool,
    ) -> Vec<Effect> {
        boxes.step(&Told {
            left,
            heard,
            changed,
            click_px: 6.0,
            size_px: SIZE,
        })
    }

    fn phase(boxes: &BoxSelect) -> Option<Phase> {
        boxes.press.as_ref().map(|press| press.phase)
    }

    use LeftKind::{Cancel, Move, Press, Release};

    #[test]
    fn a_drag_on_geometry_picks_what_was_under_the_press_and_draws_no_box() {
        let face = name(ElementKind::Face, 3);
        let mut boxes = BoxSelect::default();
        // Pressed at (50.5, 40.2), pixel (50, 40), whose hover found Face3.
        let heard = Some(([50, 40], Some(Arc::clone(&face))));
        assert!(
            frame(
                &mut boxes,
                &[event(Press, [50.5, 40.2])],
                heard.clone(),
                false
            )
            .is_empty()
        );
        // Within 6 pixels: still a click's.
        assert!(frame(&mut boxes, &[event(Move, [56.0, 40.2])], None, false).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
        // Past it: Face3 picked, replacing, and no box.
        assert_eq!(
            frame(&mut boxes, &[event(Move, [56.6, 40.2])], None, false),
            [Effect::Pick(Arc::clone(&face), How::Replace)]
        );
        assert_eq!(phase(&boxes), Some(Phase::Geometry));
        assert!(frame(&mut boxes, &[event(Move, [90.0, 70.0])], None, false).is_empty());
        assert!(frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false).is_empty());
        assert_eq!(phase(&boxes), None);
        // With Ctrl, as a Ctrl-click: toggled.
        let left = [
            event(Press, [50.5, 40.2]),
            with(event(Move, [70.0, 40.0]), false, true),
        ];
        assert_eq!(
            frame(&mut boxes, &left, heard, false),
            [Effect::Pick(face, How::Toggle)]
        );
    }

    #[test]
    fn a_drag_from_empty_space_boxes_and_its_release_selects_by_its_modifiers() {
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        let drag = |boxes: &mut BoxSelect, to: [f32; 2], shift, ctrl| {
            frame(
                boxes,
                &[event(Press, [50.0, 40.0]), event(Move, to)],
                empty.clone(),
                false,
            );
            assert_eq!(phase(boxes), Some(Phase::Boxing));
            frame(boxes, &[with(event(Release, to), shift, ctrl)], None, false)
        };
        let effect = |to: [f32; 2], crossing, how| Effect::Box {
            from: [50.0, 40.0],
            to,
            crossing,
            how,
        };
        assert_eq!(
            drag(&mut boxes, [90.0, 70.0], false, false),
            [effect([90.0, 70.0], false, How::Replace)]
        );
        assert_eq!(
            drag(&mut boxes, [10.0, 70.0], false, false),
            [effect([10.0, 70.0], true, How::Replace)]
        );
        assert_eq!(
            drag(&mut boxes, [90.0, 10.0], true, false),
            [effect([90.0, 10.0], false, How::Add)]
        );
        assert_eq!(
            drag(&mut boxes, [10.0, 10.0], false, true),
            [effect([10.0, 10.0], true, How::Toggle)]
        );
        // Both: Shift's, as the prototype's onBox takes them.
        assert_eq!(
            drag(&mut boxes, [90.0, 70.0], true, true),
            [effect([90.0, 70.0], false, How::Add)]
        );
        // Straight down, x unchanged: a window.
        assert_eq!(
            drag(&mut boxes, [50.0, 90.0], false, false),
            [effect([50.0, 90.0], false, How::Replace)]
        );
    }

    #[test]
    fn a_drag_whose_press_isnt_answered_yet_is_held_until_it_is() {
        let mut boxes = BoxSelect::default();
        // Pressed with no answer for its pixel (the newest is another's), and dragged past
        // the click distance: held, nothing decided or drawn.
        let other = Some(([3, 3], Some(name(ElementKind::Face, 1))));
        let left = [event(Press, [50.0, 40.0]), event(Move, [70.0, 40.0])];
        assert!(frame(&mut boxes, &left, other.clone(), false).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::Held));
        for _ in 0..100 {
            assert!(
                frame(
                    &mut boxes,
                    &[event(Move, [80.0, 50.0])],
                    other.clone(),
                    false
                )
                .is_empty()
            );
        }
        assert_eq!(phase(&boxes), Some(Phase::Held));
        // Answered, nothing there: a box, from the press to where the pointer is.
        assert!(frame(&mut boxes, &[], Some(([50, 40], None)), false).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::Boxing));
        assert_eq!(
            frame(&mut boxes, &[event(Release, [85.0, 55.0])], None, false),
            [Effect::Box {
                from: [50.0, 40.0],
                to: [85.0, 55.0],
                crossing: false,
                how: How::Replace,
            }]
        );
        // Again, answered with an edge there: picked.
        let edge = name(ElementKind::Edge, 5);
        frame(&mut boxes, &left, None, false);
        assert_eq!(
            frame(
                &mut boxes,
                &[],
                Some(([50, 40], Some(Arc::clone(&edge)))),
                false
            ),
            [Effect::Pick(edge, How::Replace)]
        );
        // The answer can come in the frame it's let go: decided first, then let go.
        frame(&mut boxes, &left, None, false);
        assert_eq!(
            frame(
                &mut boxes,
                &[event(Release, [20.0, 40.0])],
                Some(([50, 40], None)),
                false
            ),
            [Effect::Box {
                from: [50.0, 40.0],
                to: [20.0, 40.0],
                crossing: true,
                how: How::Replace,
            }]
        );
    }

    #[test]
    fn a_held_drag_is_let_go_when_the_view_changes_or_its_button_does() {
        let mut boxes = BoxSelect::default();
        let left = [event(Press, [50.0, 40.0]), event(Move, [70.0, 40.0])];
        frame(&mut boxes, &left, None, false);
        // The camera moved (or the bodies changed): let go, and an answer after it decides
        // nothing.
        assert!(frame(&mut boxes, &[], None, true).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::LetGo));
        assert!(frame(&mut boxes, &[], Some(([50, 40], None)), false).is_empty());
        assert!(frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false).is_empty());
        assert_eq!(phase(&boxes), None);
        // Let go of before it's answered: nothing.
        frame(&mut boxes, &left, None, false);
        assert!(frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false).is_empty());
        // A press still a click's when the view changes is let go too: its pixel's answer
        // isn't what it was pressed on.
        let empty = Some(([50, 40], None));
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0])],
            empty.clone(),
            false,
        );
        frame(&mut boxes, &[], None, true);
        assert!(frame(&mut boxes, &[event(Move, [90.0, 40.0])], empty, false).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::LetGo));
        // A box already drawn isn't: it's of the canvas, and selects at its release.
        let empty = Some(([50, 40], None));
        frame(&mut boxes, &left, empty, false);
        frame(&mut boxes, &[], None, true);
        assert_eq!(phase(&boxes), Some(Phase::Boxing));
    }

    #[test]
    fn a_cancel_ends_the_press_and_selects_nothing() {
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        let left = [
            event(Press, [50.0, 40.0]),
            event(Move, [90.0, 70.0]),
            event(Cancel, [90.0, 70.0]),
        ];
        assert!(frame(&mut boxes, &left, empty.clone(), false).is_empty());
        assert_eq!(phase(&boxes), None);
        // A stray release after it is nothing.
        assert!(frame(&mut boxes, &[event(Release, [90.0, 70.0])], empty, false).is_empty());
    }

    #[test]
    fn a_release_past_the_click_distance_with_no_move_between_is_a_drag() {
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        let left = [event(Press, [50.0, 40.0]), event(Release, [50.0, 46.5])];
        assert_eq!(
            frame(&mut boxes, &left, empty.clone(), false),
            [Effect::Box {
                from: [50.0, 40.0],
                to: [50.0, 46.5],
                crossing: false,
                how: How::Replace,
            }]
        );
        // Within it: a click, the selection's alone.
        let left = [event(Press, [50.0, 40.0]), event(Release, [50.0, 46.0])];
        assert!(frame(&mut boxes, &left, empty, false).is_empty());
    }

    // ---- Hostile input (break-it) ----

    #[test]
    fn a_malformed_mesh_takes_nothing_it_cant_read_and_never_panics() {
        // Counts past the arrays, an index past the positions, a point that isn't finite: a
        // face or an edge with a point that can't be read isn't wholly inside; a crossing
        // tests what it can read.
        let mut held = body(&[], &[]);
        held.mesh = Arc::new(Mesh {
            positions: vec![
                at(110.0, 20.0, 0.0),
                at(150.0, 20.0, 0.0),
                at(150.0, 40.0, 0.0),
            ],
            normals: vec![[0.0, -1.0, 0.0]; 3],
            triangles: vec![[0, 1, 2], [0, 1, 99]],
            face_triangles: vec![1, 1, u32::MAX],
            valid: None,
            edge_points: vec![
                at(110.0, 30.0, 0.0),
                [f64::NAN, 0.0, 0.0],
                at(120.0, 30.0, 0.0),
                at(130.0, 30.0, 0.0),
            ],
            edge_point_counts: vec![2, 2, u32::MAX],
        });
        held.faces = (1..=3).map(|n| named(ElementKind::Face, n)).collect();
        held.edges = (1..=3).map(|n| named(ElementKind::Edge, n)).collect();
        let held = bodies(held);
        let all = Kinds {
            faces: true,
            edges: true,
        };
        // Face1 whole; Face2's third corner is no position; Face3 runs past the triangles.
        // Edge1 has a point that isn't finite; Edge2 is whole; Edge3 runs past the points.
        assert_eq!(
            boxed(&held, all, [100.0, 10.0], [160.0, 50.0]),
            ["Face1", "Edge2"]
        );
        // A crossing takes Face2 by its corners that are read, and Edge1 by its point that
        // projects.
        assert_eq!(
            boxed(&held, all, [160.0, 10.0], [100.0, 50.0]),
            ["Face1", "Face2", "Edge1", "Edge2"]
        );
        // More names than elements, or fewer: what has no name isn't taken.
        let mut fewer = held[&1].clone();
        fewer.faces.truncate(1);
        fewer.edges.clear();
        assert_eq!(
            boxed(&bodies(fewer), all, [160.0, 10.0], [100.0, 50.0]),
            ["Face1"]
        );
    }

    #[test]
    fn an_empty_scene_a_box_of_no_size_and_a_camera_that_cant_draw_take_nothing() {
        let edges: [&[[f64; 2]]; 1] = [&[[110.0, 30.0], [130.0, 30.0]]];
        let held = bodies(body(&edges, &[]));
        assert!(boxed(&BTreeMap::new(), EDGES, [0.0, 0.0], [256.0, 128.0]).is_empty());
        // A box of no size takes only what's exactly on its point: an edge of one point
        // there, not one through it.
        assert!(boxed(&held, EDGES, [120.0, 30.0], [120.0, 30.0]).is_empty());
        let single: [&[[f64; 2]]; 1] = [&[[120.0, 31.0]]];
        assert_eq!(
            boxed(
                &bodies(body(&single, &[])),
                EDGES,
                [120.0, 31.0],
                [120.0, 31.0]
            ),
            ["Edge1"]
        );
        // A camera the renderer refuses, or a canvas of no size: nothing projects.
        let mut camera = front();
        camera.eye = camera.target;
        let area = PxBox::clamped([0.0, 0.0], [256.0, 128.0], SIZE);
        assert!(taken(&held, &every, EDGES, &camera, SIZE, area, true).is_empty());
        assert!(taken(&held, &every, EDGES, &front(), [0, 0], area, true).is_empty());
    }

    #[test]
    fn a_box_over_a_large_model_takes_every_edge_inside_once_in_order() {
        // 100,000 edges, each a two-point polyline half a pixel long, in a 200 by 100 grid
        // of pixels a quarter in: a window to x = 99.9 takes those wholly in it, in their
        // order.
        let mut held = body(&[], &[]);
        let count = 100_000u32;
        let mut points = Vec::with_capacity(2 * count as usize);
        for n in 0..count {
            let x = f64::from(n % 200) + 0.25;
            let y = f64::from(n / 200 % 100) + 0.25;
            points.push(at(x, y, 0.0));
            points.push(at(x + 0.5, y, 0.0));
        }
        held.mesh = Arc::new(Mesh {
            edge_points: points,
            edge_point_counts: vec![2; count as usize],
            ..(*held.mesh).clone()
        });
        held.edges = (1..=count).map(|n| named(ElementKind::Edge, n)).collect();
        let held = bodies(held);
        let names = boxed(&held, EDGES, [0.0, 0.0], [99.9, 100.0]);
        // By hand: x from 0 to 99 (the edge's end at x + 0.75 ≤ 99.9), each of the 100 rows,
        // over the 5 times the grid repeats: 100 × 100 × 5.
        assert_eq!(names.len(), 50_000);
        assert_eq!(names[0], "Edge1");
        assert_eq!(names[1], "Edge2");
        assert_eq!(names[100], "Edge201");
    }

    #[test]
    fn a_press_held_without_a_release_and_a_second_press_starts_again() {
        // A press whose release never came (the window lost one): the next press begins
        // afresh, the first's decision dropped.
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0]), event(Move, [90.0, 70.0])],
            empty,
            false,
        );
        assert_eq!(phase(&boxes), Some(Phase::Boxing));
        frame(&mut boxes, &[event(Press, [10.0, 10.0])], None, false);
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
        // Positions that aren't finite never pass the click distance, nor decide.
        assert!(
            frame(
                &mut boxes,
                &[event(Move, [f32::NAN, f32::INFINITY])],
                None,
                false
            )
            .is_empty()
        );
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
        assert!(frame(&mut boxes, &[event(Release, [f32::NAN, 0.0])], None, false).is_empty());
        assert_eq!(phase(&boxes), None);
    }
}
