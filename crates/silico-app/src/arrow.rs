//! A feature dialog's arrows in the canvas (the prototype audit's item 7; docs/design/m5.0-app.md,
//! section 8, *Feature dialogs*: "a distance input shows a draggable arrow in the canvas, and a
//! direction input an arrow that flips", Fusion's in-canvas manipulators, docs/ux/fusion.md).
//!
//! **The distance arrow** is the approved prototype's (docs/design/m5-ui/js/interact.js:
//! `arrowGeom`, `arrowHit`, `onPointerDown`, `onPointerDrag`, `onPointerUp`, and the overlay).
//! While a pad's dialog is open on a sketch, an arrow stands at the end of the sweep, at the
//! profile's centroid moved the distance along the sweep (the prototype's `(18, D, 14)`: its
//! bracket's L profile's centroid, (18.2, 13.2), to round numbers), pointing along the sweep,
//! [`theme::ARROW_LENGTH`] points long on the screen whatever the zoom, with its head, a dot
//! over its base, and its value in mm in a box past its head; white while it's dragged.
//!
//! A press near it (within 14 points of its shaft's end, or 6 of its shaft) takes the left
//! button's stream, which neither the selection nor the box then sees. Each move of the stream
//! takes the pointer's move from the press along the arrow's direction on the screen, over the
//! arrow's points a millimetre along the sweep (both as they were at the press, measured over
//! 10 mm of the sweep at its base), as the millimetres its end moved; and sets the input to the
//! value it held at the press moved by that, to the nearest half millimetre (a half up, as
//! `Math.round`), never under half a millimetre: the prototype's
//! `Math.max(0.5, Math.round((D0 + dd) * 2) / 2)`, set only where it differs from the input's.
//! So a drag past the profile stops at 0.5 mm and never turns the sweep over. But where the
//! closed span from the value at the press to where the drag takes it holds no half
//! millimetre a drag can land on (from 0.5 up), the value stays exactly as it was (Main's rule
//! in #480's review; the prototype puts it on the half millimetres at the first move), worked
//! in whole nanometres: a value on the grid moves as the prototype's does, and one off it,
//! 10.3, stays 10.3 until the drag reaches 10.5 (then 10.5, and 11 from 10.75) or 10 (then 10,
//! and 9.5 from 9.75), and is 10.3 again when the drag comes back. A move square across the
//! arrow never rewrites a value, and a drag never moves one against its own way (from 0.3,
//! down to 0 leaves 0.3).
//! Nothing caps a drag above: a value past what a sweep takes is the core's to refuse, as a
//! typed one is. An arrow whose sweep the view foreshortens to under
//! 0.6 points a millimetre (seen end on, or zoomed far out) isn't drawn and takes no press, as
//! the prototype's `flat` one.
//!
//! The prototype draws a pad's alone: its pocket goes through all, with no distance. Where it's
//! silent, SolidWorks' extrude handles are followed (docs/ux/solidworks.md, §5: "dragging a
//! handle shows a dynamic preview"; Fusion gives every distance input one): a pocket's length
//! has its arrow too, along the pocket's way, against the sketch's normal; a symmetric extent's
//! one arrow stands at half its length, its value moving by twice its end's move; a two-sided
//! extent has one for each length, the second's against the sweep. Through all has none.
//! Where each length ends, and the way the sweep goes, are the core's
//! ([`silico_core::sweep::prism`], of the extent the dialog's fields read as, by the reader a
//! feature's extent is read by, [`silico_core::document::read_extent`]): the rule the rebuild's
//! prism step reads, so an arrow stands where the preview builds. While a field the extent shows
//! doesn't read, there's no distance arrow, as there's no sweep to build.
//! Revolve's and groove's angle would take Fusion's angle widget, not an arrow: none is drawn.
//!
//! **The flip arrow** is Fusion's direction input's (docs/ux/fusion.md, *Feature dialogs*: "a
//! direction input shows an arrow that flips"); the prototype has none, its Direction being the
//! dialog's two buttons (app.js's `dlgRev`). It stands at the profile's centroid on the sketch's
//! plane, pointing the way the sweep goes, drawn as the distance arrow is, half as long, with no
//! dot or label, and shows while the dialog's Reversed does (one side and through all), as a
//! Fusion manipulator hides while its input is hidden. A click on it, its press and its release
//! both on it, turns Reversed over, and nothing else: `dlgRev` sets `rev` alone.
//!
//! An arrow changes only the dialog's inputs, as typing does: the tab sends what changed as the
//! dialog's edits through the command API (`Tab::dialog_changed`), at most one in flight and the
//! newest waiting in place of the one before (section 3, *Bursts of edits*), so a drag's moves
//! never queue. And while a drag is under way, its newest value waits in the field (which still
//! follows the pointer) until the last edit's preview has answered, so a steady drag keeps one
//! preview rebuilding, as the sketch's drag keeps one solve in flight, rather than stopping and
//! starting a rebuild each frame (`Tab::arrow_held_back`). The drag's end sends the value it
//! held back in the release's frame, its tab shown or not (`Tab::arrows_frame`); OK, and a save
//! that closes the tab, end a drag still under way by sending it first (`Tab::close_dialog`,
//! `Tab::flush_arrow`). Esc ends a drag where it began (section 3's chain, an open drag first:
//! `Tab::escape_first`), as the stream's cancel does; a dialog closed, or its tab beginning to
//! close, mid-drag leaves the rest of its stream doing nothing.
//!
//! Each arrow is data first, in model space ([`handles`], from the dialog, the sketch's plane,
//! and its profile's centroid, [`profile_centroid`]) and then on the screen ([`arrows`], through
//! the frame's camera by the renderer's own projection), turned into egui's shapes by one
//! function ([`Arrows::shapes`]), so it's tested without a GPU, as the triad is.

use std::sync::Arc;

use eframe::egui::{
    CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2,
};
use silico_core::command::sketch::SketchRead;
use silico_core::document::{Length, SweepFeature, read_extent};
use silico_core::kernel::SketchPlane;
use silico_core::sweep;
use silico_protocol::Camera;
use silico_render::render::project;

use crate::dialog::{self, Dialog, Kind, Part, Value};
use crate::sketch::draw::{Geometry, add, dot, length, mul, oriented, segment_distance, sub};
use crate::theme;
use crate::viewport::{LeftEvent, LeftKind};

/// How far along the sweep an arrow's way on the screen is measured, in mm
/// (`base.clone().addScaledVector(dir, 10)`).
pub const PROBE_MM: f64 = 10.0;

/// The fewest points a millimetre along the sweep an arrow is drawn at (`ppu < 0.6` is
/// `flat`).
pub const FLAT: f64 = 0.6;

/// Half a millimetre, in nanometres: the grid a drag's value lands on, where it leaves the
/// value at its press (`Math.max(0.5, Math.round(v * 2) / 2)`), each value of it a whole
/// number of them from one up.
pub const HALF_MM_NM: i64 = (crate::properties::NM_PER_MM / 2) as i64;

/// A length of the extent a distance arrow drags, in model space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    /// The input it drags: the extent's `length`, or a two-sided extent's `length2`.
    pub part: Part,
    /// What the input holds.
    pub value: Length,
    /// Where it stands: the end of the sweep the value sets.
    pub base: [f64; 3],
    /// The way it points, a unit vector: away from the profile, along the sweep (against it
    /// for `length2`).
    pub way: [f64; 3],
    /// How far the value moves for each millimetre its end does: 1, or 2 for a symmetric
    /// extent's length, half of which lies each side of the profile.
    pub per_mm: f64,
}

/// A dialog's arrows in model space.
#[derive(Debug, Clone, PartialEq)]
pub struct Handles {
    /// The profile's centroid on the sketch's plane: where the sweep starts, and the flip
    /// arrow stands.
    pub start: [f64; 3],
    /// The way the sweep goes, a unit vector: the sketch's normal for a pad, against it for a
    /// pocket, the other way when reversed (docs/design/m3.6-sketch-features.md, *Directions*).
    pub direction: [f64; 3],
    /// A distance arrow for each of the extent's lengths above 0, at its end of the core's
    /// prism ([`silico_core::sweep::prism`]); none while a field the extent shows doesn't read,
    /// as the extent the core would read is then none. A length that reads but isn't above 0
    /// still reads as an extent (the gate refuses it later, as it does a typed one), so the
    /// other length keeps its arrow; it has none of its own, a sweep of no length having no
    /// end to drag away from the profile.
    pub distances: Vec<Distance>,
    /// Whether the flip arrow shows: while the dialog's Reversed does.
    pub flip: bool,
}

/// The centroid of `read`'s profile, in the sketch's millimetres: of every region's area, its
/// outer loop's less its holes'. Each loop is its curves as the walk of its outline runs them
/// ([`oriented`]), sent from the read's stored points by the core's own conversion of a
/// profile's curve ([`silico_core::profile::sent`]), and measured exactly by the request's
/// own measure ([`silico_protocol::area_moments`], beside its area and perimeter). None for a
/// sketch that makes no profile, a loop that doesn't run end to end or whose arc makes none,
/// or a profile whose area isn't above 0.
pub fn profile_centroid(read: &SketchRead) -> Option<[f64; 2]> {
    let geometry = Geometry::of(read);
    // Each loop about a point of its own, then all about the first's: a profile far from the
    // sketch's origin, or a loop far from the others, keeps the precision its own size has.
    let mut about: Option<[f64; 2]> = None;
    let (mut area, mut moment) = (0.0, [0.0, 0.0]);
    for region in &read.regions {
        let loops = std::iter::once((&region.outer, 1.0))
            .chain(region.holes.iter().map(|hole| (hole, -1.0)));
        for (curves, role) in loops {
            // The walk's curves, in `curves`' order, each as the core's profile sends it.
            let sent: Vec<silico_protocol::Curve> = curves
                .iter()
                .zip(oriented(curves, &geometry)?)
                .map(|(id, walked)| {
                    silico_core::profile::sent(geometry.profile_curve(id)?, walked.backwards)
                })
                .collect::<Option<_>>()?;
            let measured = silico_protocol::area_moments(&sent);
            let shift = sub(measured.about, *about.get_or_insert(measured.about));
            // However a loop runs (a lone circle counter-clockwise, a hole or not), it counts by
            // what it is: an outer loop's area added, a hole's taken away.
            let sense = role * measured.area.signum();
            area += sense * measured.area;
            let moment_here = add(measured.moment, mul(shift, measured.area));
            moment = add(moment, mul(moment_here, sense));
        }
    }
    let centroid = add(about?, mul(moment, 1.0 / area));
    (area > 0.0 && centroid.iter().all(|c| c.is_finite())).then_some(centroid)
}

/// `dialog`'s arrows in model space, its sketch on `plane` with its profile's centroid at
/// `centroid`: a pad's and a pocket's, as the module says; none for another kind, or a dialog
/// with no extent. Its sweep is the core's prism of the extent the dialog's shown fields read
/// as: none while one of them doesn't read, and a length not above 0 has no arrow.
pub fn handles(dialog: &Dialog, plane: SketchPlane, centroid: [f64; 2]) -> Option<Handles> {
    let kind = match dialog.kind {
        Kind::Pad => SweepFeature::Pad,
        Kind::Pocket => SweepFeature::Pocket,
        _ => return None,
    };
    let reversed = dialog
        .inputs
        .iter()
        .find(|input| input.part == Part::Reversed && dialog.shown(input))
        .map(|input| matches!(input.value, Value::Toggle(true)));
    let start = plane.place(centroid);
    if !finite3(start) {
        return None;
    }
    // The sweep the rebuild would make of the extent the dialog shows: none while a field of
    // it doesn't read, whose way the flip arrow still shows.
    let prism = dialog
        .fields()
        .remove("extent")?
        .and_then(|text| read_extent(kind, &text).ok())
        .and_then(|extent| sweep::prism(extent, plane.normal()));
    let direction = match &prism {
        Some(prism) => prism.direction,
        None => sweep::prism_direction(kind, reversed == Some(true), plane.normal()),
    };
    let distances = prism
        .map(|prism| prism.ends)
        .unwrap_or_default()
        .into_iter()
        .filter(|end| end.length.nm() > 0)
        .filter_map(|end| {
            let base: [f64; 3] = std::array::from_fn(|k| start[k] + end.offset[k]);
            finite3(base).then_some(Distance {
                part: Part::ExtentNumber(end.field),
                value: end.length,
                base,
                way: end.way,
                per_mm: end.per_mm,
            })
        })
        .collect();
    Some(Handles {
        start,
        direction,
        distances,
        flip: reversed.is_some(),
    })
}

fn finite3(p: [f64; 3]) -> bool {
    p.iter().all(|c| c.is_finite())
}

/// An arrow on the screen, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrow {
    /// Where it starts: the place it stands at, on the screen.
    pub base: Pos2,
    /// The way it points on the screen, a unit vector, the screen's y down.
    pub unit: [f64; 2],
    /// Its points a millimetre along the sweep at its base, measured over [`PROBE_MM`]
    /// (`ppu`).
    pub points_per_mm: f64,
    /// Its shaft's end, its length past its base.
    pub tip: Pos2,
    /// Its head: its point, [`theme::ARROW_HEAD_LENGTH`] past the shaft's end, then its
    /// corners, [`theme::ARROW_HEAD_HALF_WIDTH`] either side of the shaft's end (the left of
    /// its way first, the screen's y down).
    pub head: [Pos2; 3],
}

impl Arrow {
    /// The arrow from `base` along `unit` on the screen (a unit vector), `points_per_mm` points a
    /// millimetre, its shaft `length` points; none where a place of it isn't finite.
    fn new(base: [f64; 2], unit: [f64; 2], points_per_mm: f64, length: f32) -> Option<Arrow> {
        let along = |from: [f64; 2], by: f32| add(from, mul(unit, f64::from(by)));
        let tip = along(base, length);
        let across = [-unit[1], unit[0]];
        let side = |k: f64| {
            point(add(
                tip,
                mul(across, f64::from(theme::ARROW_HEAD_HALF_WIDTH) * k),
            ))
        };
        Some(Arrow {
            base: point(base)?,
            unit,
            points_per_mm,
            tip: point(tip)?,
            head: [
                point(along(tip, theme::ARROW_HEAD_LENGTH))?,
                side(1.0)?,
                side(-1.0)?,
            ],
        })
    }

    /// Whether a press at `at` takes it (`arrowHit`): nearer than [`theme::ARROW_HIT_TIP`]
    /// to its shaft's end, or than [`theme::ARROW_HIT_SHAFT`] to its shaft.
    pub fn hit(&self, at: Pos2) -> bool {
        let at = wide(at);
        let (base, tip) = (wide(self.base), wide(self.tip));
        length(sub(at, tip)) < f64::from(theme::ARROW_HIT_TIP)
            || segment_distance(at, base, tip) < f64::from(theme::ARROW_HIT_SHAFT)
    }

    /// Its shaft and its head, in `colour`.
    fn shapes(&self, colour: eframe::egui::Color32) -> [Shape; 2] {
        [
            Shape::line_segment(
                [self.base, self.tip],
                Stroke::new(theme::ARROW_WIDTH, colour),
            ),
            Shape::convex_polygon(self.head.to_vec(), colour, Stroke::NONE),
        ]
    }
}

/// `p` as a point on the screen, if it's finite there.
fn point(p: [f64; 2]) -> Option<Pos2> {
    let at = pos2(p[0] as f32, p[1] as f32);
    at.is_finite().then_some(at)
}

fn wide(p: Pos2) -> [f64; 2] {
    [f64::from(p.x), f64::from(p.y)]
}

/// A distance arrow on the screen, with what it drags and its label.
#[derive(Debug, Clone, PartialEq)]
pub struct DistanceArrow {
    pub distance: Distance,
    pub arrow: Arrow,
    /// Its value in mm, as the prototype's `fmt` writes it ("12.5 mm", "1,250 mm").
    pub label: String,
    /// Its label box's left, at its middle down (`lx`, `ly`).
    pub label_at: Pos2,
}

/// What a press took, or the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The distance arrow of the input `part`.
    Distance(Part),
    Flip,
}

/// A dialog's arrows on the screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arrows {
    pub distances: Vec<DistanceArrow>,
    pub flip: Option<Arrow>,
}

/// `handles` on the screen through `camera`, the image `size_px` in pixels at
/// `pixels_per_point`, its top left at `origin` in points: each where the renderer draws its
/// place ([`project`]), its way measured from there to [`PROBE_MM`] along it, and left out
/// where the view foreshortens that under [`FLAT`] points a millimetre or can't show either
/// place (behind a perspective eye).
pub fn arrows(
    handles: &Handles,
    camera: &Camera,
    size_px: [u32; 2],
    pixels_per_point: f32,
    origin: Pos2,
) -> Arrows {
    let ppp = f64::from(pixels_per_point);
    let on_screen = |at: [f64; 3]| -> Option<[f64; 2]> {
        let [x, y] = project(camera, size_px, at)?;
        let p = add(wide(origin), mul([x, y], 1.0 / ppp));
        (ppp > 0.0 && p.iter().all(|c| c.is_finite())).then_some(p)
    };
    let arrow = |base: [f64; 3], way: [f64; 3], shaft: f32| -> Option<Arrow> {
        let a = on_screen(base)?;
        let b = on_screen(std::array::from_fn(|k| base[k] + PROBE_MM * way[k]))?;
        let d = sub(b, a);
        let per_mm = length(d) / PROBE_MM;
        if !(per_mm >= FLAT && per_mm.is_finite()) {
            return None;
        }
        Arrow::new(a, mul(d, 1.0 / (per_mm * PROBE_MM)), per_mm, shaft)
    };
    let distances = handles
        .distances
        .iter()
        .filter_map(|distance| {
            let arrow = arrow(distance.base, distance.way, theme::ARROW_LENGTH)?;
            let past = add(
                wide(arrow.tip),
                mul(arrow.unit, f64::from(theme::ARROW_LABEL_PAST)),
            );
            let label_at = point(add(past, [f64::from(theme::ARROW_LABEL_SHIFT), 0.0]))?;
            Some(DistanceArrow {
                distance: *distance,
                arrow,
                label: label(distance.value),
                label_at,
            })
        })
        .collect();
    let flip = handles
        .flip
        .then(|| arrow(handles.start, handles.direction, theme::FLIP_ARROW_LENGTH))
        .flatten();
    Arrows { distances, flip }
}

/// `value` in mm as the prototype's label writes it (`A.fmt(doc().p.D) + ' mm'`): to at most
/// two decimals, its whole part grouped by threes.
pub fn label(value: Length) -> String {
    format!(
        "{} mm",
        crate::properties::trimmed(i128::from(value.nm()), crate::properties::NM_PER_MM)
    )
}

impl Arrows {
    /// Whether it has no arrow to draw.
    pub fn is_empty(&self) -> bool {
        self.distances.is_empty() && self.flip.is_none()
    }

    /// The arrow a press at `at` takes, if one does: a distance arrow first, in their order,
    /// then the flip arrow.
    pub fn hit(&self, at: Pos2) -> Option<Hit> {
        self.distances
            .iter()
            .find(|distance| distance.arrow.hit(at))
            .map(|distance| Hit::Distance(distance.distance.part))
            .or_else(|| self.flip.filter(|flip| flip.hit(at)).map(|_| Hit::Flip))
    }

    /// Its shapes, in the order they're painted: the flip arrow's shaft and head, then each
    /// distance arrow's shaft, head, base's dot, and label (its box filled, its outline, its
    /// words), as the prototype's overlay paints its one; each in [`theme::ARROW`], but the one
    /// `held` in [`theme::ARROW_HELD`].
    pub fn shapes(&self, painter: &Painter, held: Option<Hit>) -> Vec<Shape> {
        let _span = tracing::debug_span!("dialog_arrows").entered();
        let colour = |hit: Hit| {
            if held == Some(hit) {
                theme::ARROW_HELD
            } else {
                theme::ARROW
            }
        };
        let mut shapes = Vec::new();
        if let Some(flip) = &self.flip {
            shapes.extend(flip.shapes(colour(Hit::Flip)));
        }
        for distance in &self.distances {
            let colour = colour(Hit::Distance(distance.distance.part));
            let arrow = &distance.arrow;
            shapes.extend(arrow.shapes(colour));
            shapes.push(Shape::circle_filled(
                arrow.base,
                theme::ARROW_DOT_RADIUS,
                colour,
            ));
            let font = FontId::proportional(theme::ARROW_LABEL_SIZE);
            let galley =
                painter.layout_no_wrap(distance.label.clone(), font, theme::ARROW_LABEL_TEXT);
            let at = distance.label_at;
            let frame = Rect::from_min_size(
                pos2(at.x, at.y - theme::ARROW_LABEL_HEIGHT / 2.0),
                vec2(
                    galley.size().x + 2.0 * theme::ARROW_LABEL_PAD,
                    theme::ARROW_LABEL_HEIGHT,
                ),
            );
            let radius = CornerRadius::same(theme::ARROW_LABEL_RADIUS);
            shapes.push(Shape::rect_filled(frame, radius, theme::ARROW_LABEL_FILL));
            shapes.push(Shape::rect_stroke(
                frame,
                radius,
                Stroke::new(theme::ARROW_LABEL_OUTLINE, colour),
                StrokeKind::Middle,
            ));
            let corner = pos2(
                frame.left() + theme::ARROW_LABEL_PAD,
                at.y + theme::ARROW_LABEL_DROP - crate::triad::baseline(&galley),
            );
            shapes.push(Shape::galley(corner, galley, theme::ARROW_LABEL_TEXT));
        }
        shapes
    }
}

/// The pointer's cursor over an arrow: the prototype's `grab` over a distance arrow (and
/// through its drag, which its hover leaves as it was); a pointing hand over the flip arrow,
/// which a click acts on.
pub fn cursor(hit: Hit) -> CursorIcon {
    match hit {
        Hit::Distance(_) => CursorIcon::Grab,
        Hit::Flip => CursorIcon::PointingHand,
    }
}

/// The value a drag of `distance`'s arrow gives, the arrow as `arrow` was at the press at
/// `press`, the pointer now at `at` (`onPointerDrag`): the pointer's move along the arrow's way
/// on the screen over its `points_per_mm` is how far the end moved, in mm; times `distance`'s
/// `per_mm`, that's how far the value moves, to the nanometre, to `t`; and the value is
/// [`stepped`]'s from the value at the press to `t`. None where that isn't a length (a move
/// past what one holds, or one that isn't a number).
pub fn dragged(distance: &Distance, arrow: &Arrow, press: Pos2, at: Pos2) -> Option<Length> {
    let moved = dot(sub(wide(at), wide(press)), arrow.unit) / arrow.points_per_mm;
    let by = distance.per_mm * moved * crate::properties::NM_PER_MM as f64;
    if !by.is_finite() {
        return None;
    }
    // Taken to the nanometre, saturating: a move past any length's reach is past it either
    // way, down to the least step or up to none.
    let to = i128::from(distance.value.nm()).saturating_add(by.round() as i128);
    stepped(distance.value, to)
}

/// The value a drag from `from` to `to` (in nanometres) gives (Main's rule in #480's review):
/// `from`, exactly, where the closed span between them holds no value a drag can land on, a
/// half millimetre from 0.5 up; else `to` on the half millimetres, as the prototype puts a
/// drag's value ([`on_the_grid`]). So a value on the grid, which its span always holds, moves
/// as the prototype's does; one off it, 10.3, stays until the drag reaches a half millimetre,
/// 10.5 or 10, and comes back to itself when the drag does; and a drag never moves a value
/// against its own way: from 0.3, down to 0 or below leaves 0.3, and up to 0.5 gives 0.5.
/// Worked in whole nanometres, so each edge is exact.
fn stepped(from: Length, to: i128) -> Option<Length> {
    let half = i128::from(HALF_MM_NM);
    let at = i128::from(from.nm());
    // The least value a drag lands on is half a millimetre: none below it is in the span.
    let (low, high) = (at.min(to).max(half), at.max(to));
    // The first half millimetre at or above `low` (`low` is at most `from` or `half`, so it
    // fits).
    let first = low.div_euclid(half) + i128::from(low.rem_euclid(half) != 0);
    if first * half > high {
        return Some(from);
    }
    on_the_grid(to)
}

/// `nm` on the half millimetres, as the prototype puts a drag's value
/// (`Math.max(0.5, Math.round(v * 2) / 2)`): to the nearest, a half up, and at least half a
/// millimetre. None past what a length holds.
fn on_the_grid(nm: i128) -> Option<Length> {
    let half = i128::from(HALF_MM_NM);
    let halves = nm.saturating_add(half / 2).div_euclid(half).max(1);
    i64::try_from(halves.checked_mul(half)?)
        .ok()
        .map(Length::from_nm)
}

/// What the arrows ask of the dialog's inputs.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// A drag's value for the input `part`: set where it holds another.
    Drag(Part, Length),
    /// The input `part` given back the text it held at the press: a drag cancelled.
    Restore(Part, String),
    /// Reversed turned over.
    Flip,
}

/// What `asked` asks, done to `dialog`'s inputs: a drag's value set where the input reads as
/// another length (`v !== doc().p.D`), written as the document writes it ("12.5"); a drag's
/// input given back its text; Reversed turned over. Whether an input changed.
pub fn apply(dialog: &mut Dialog, asked: Vec<Asked>) -> bool {
    let mut changed = false;
    for ask in asked {
        let part = match &ask {
            Asked::Drag(part, _) | Asked::Restore(part, _) => *part,
            Asked::Flip => Part::Reversed,
        };
        let Some(input) = dialog.inputs.iter_mut().find(|input| input.part == part) else {
            continue;
        };
        match (ask, &mut input.value) {
            (Asked::Drag(_, value), Value::Length(text)) => {
                let now = dialog::number(text).and_then(|number| Length::parse(&number).ok());
                if now != Some(value) {
                    *text = value.to_string();
                    changed = true;
                }
            }
            (Asked::Restore(_, was), Value::Length(text)) => {
                if *text != was {
                    *text = was;
                    changed = true;
                }
            }
            (Asked::Flip, Value::Toggle(on)) => {
                *on = !*on;
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

/// The left button's stream an arrow's press took, from the press to its release or cancel,
/// all of it the arrow's.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    /// The dialog it was pressed in.
    serial: u64,
    /// Where it was pressed, in points, and in the pane's pixels (where the selection's hover
    /// stays while it's held, as the prototype's hover waits out a drag).
    press: Pos2,
    press_px: [f32; 2],
    took: Took,
    /// Whether it still acts: not once the dialog it was pressed in has closed, or Esc has
    /// ended it.
    live: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Took {
    /// A distance arrow, as it was at the press, and the text its input held then.
    Distance {
        distance: Distance,
        arrow: Arrow,
        was: String,
    },
    /// The flip arrow, as it was at the press.
    Flip(Arrow),
}

/// A tab's arrows between frames: the stream one took, if one did, and its profile's centroid,
/// kept with the sketch's read it was worked out of.
#[derive(Debug, Default)]
pub struct Grip {
    held: Option<Held>,
    /// Whether the last release was an arrow's: the double-click straight after it is its too.
    released: bool,
    centroid: Option<(Arc<SketchRead>, Option<[f64; 2]>)>,
}

impl Grip {
    /// The left button's `events` of a frame, an arrow's taken and the rest given back, in
    /// order; and what they ask of the dialog's inputs, in order. `arrows` are the open
    /// `dialog`'s as this frame shows them, the events' places in the pane's pixels at
    /// `pixels_per_point` from its top left at `origin` in points.
    ///
    /// A press on an arrow takes its stream: its moves drag a distance arrow ([`dragged`]), its
    /// release on the flip arrow, where it was pressed, turns Reversed over, and its cancel
    /// gives a dragged input back the text it held at the press. A stream an arrow didn't take
    /// is given back whole, and so is a press with no dialog. Once the dialog it was pressed in
    /// has closed, the rest of the stream asks nothing.
    pub fn frame(
        &mut self,
        events: &[LeftEvent],
        arrows: Option<&Arrows>,
        dialog: Option<&Dialog>,
        pixels_per_point: f32,
        origin: Pos2,
    ) -> (Vec<LeftEvent>, Vec<Asked>) {
        let serial = dialog.map(|dialog| dialog.serial);
        if let Some(held) = &mut self.held
            && Some(held.serial) != serial
        {
            held.live = false;
        }
        let mut passed = Vec::new();
        let mut asked = Vec::new();
        for event in events {
            let at = crate::sketch::at_points(event.at_px, pixels_per_point, origin);
            match event.kind {
                LeftKind::Press => {
                    self.released = false;
                    // The viewport ends every stream before the next press; one still held
                    // here lost its end, and goes.
                    self.held = dialog.zip(arrows).and_then(|(dialog, arrows)| {
                        Some(Held {
                            serial: dialog.serial,
                            press: at,
                            press_px: event.at_px,
                            took: took(arrows, dialog, at)?,
                            live: true,
                        })
                    });
                    if self.held.is_none() {
                        passed.push(*event);
                    }
                }
                LeftKind::Move => match &self.held {
                    Some(Held {
                        live: true,
                        press,
                        took:
                            Took::Distance {
                                distance, arrow, ..
                            },
                        ..
                    }) => {
                        if let Some(value) = dragged(distance, arrow, *press, at) {
                            asked.push(Asked::Drag(distance.part, value));
                        }
                    }
                    Some(_) => {}
                    None => passed.push(*event),
                },
                LeftKind::Release => {
                    self.released = self.held.is_some();
                    match self.held.take() {
                        Some(Held {
                            live: true,
                            took: Took::Flip(flip),
                            ..
                        }) if flip.hit(at) => asked.push(Asked::Flip),
                        Some(_) => {}
                        None => passed.push(*event),
                    }
                }
                LeftKind::Cancel => match self.held.take() {
                    Some(Held {
                        live: true,
                        took: Took::Distance { distance, was, .. },
                        ..
                    }) => asked.push(Asked::Restore(distance.part, was)),
                    Some(_) => {}
                    None => passed.push(*event),
                },
                LeftKind::DoubleClick => {
                    if !std::mem::take(&mut self.released) {
                        passed.push(*event);
                    }
                }
            }
        }
        (passed, asked)
    }

    /// Esc, given to the stream held if one is and still acts in the dialog numbered
    /// `serial` (section 3's chain, an open drag first): it ends, a drag where it began, and
    /// the rest of its stream asks nothing. What it asks, if it took the Esc.
    pub fn escape(&mut self, serial: Option<u64>) -> Option<Vec<Asked>> {
        let held = self
            .held
            .as_mut()
            .filter(|held| held.live && Some(held.serial) == serial)?;
        held.live = false;
        Some(match &held.took {
            Took::Distance { distance, was, .. } => {
                vec![Asked::Restore(distance.part, was.clone())]
            }
            Took::Flip(_) => Vec::new(),
        })
    }

    /// The input a distance arrow's drag under way in the dialog numbered `serial` drags,
    /// while its stream still acts.
    pub fn dragging(&self, serial: Option<u64>) -> Option<Part> {
        let held = self
            .held
            .as_ref()
            .filter(|held| held.live && Some(held.serial) == serial)?;
        match &held.took {
            Took::Distance { distance, .. } => Some(distance.part),
            Took::Flip(_) => None,
        }
    }

    /// The stream held let go of, if one is: what's left of it asks nothing (its dialog
    /// closing by OK or Cancel).
    pub fn let_go(&mut self) {
        if let Some(held) = &mut self.held {
            held.live = false;
        }
    }

    /// Whether an arrow holds the left button's stream, acting or not.
    pub fn holding(&self) -> bool {
        self.held.is_some()
    }

    /// The arrow held, while its stream still acts.
    pub fn held(&self) -> Option<Hit> {
        let held = self.held.as_ref().filter(|held| held.live)?;
        Some(match &held.took {
            Took::Distance { distance, .. } => Hit::Distance(distance.part),
            Took::Flip(_) => Hit::Flip,
        })
    }

    /// Where the stream held was pressed, in the pane's pixels, while one is held.
    pub fn press_px(&self) -> Option<[f32; 2]> {
        self.held.as_ref().map(|held| held.press_px)
    }

    /// `read`'s profile's centroid ([`profile_centroid`]), worked out once a read.
    pub fn centroid(&mut self, read: &Arc<SketchRead>) -> Option<[f64; 2]> {
        match &self.centroid {
            Some((kept, centroid)) if Arc::ptr_eq(kept, read) => *centroid,
            _ => {
                let centroid = profile_centroid(read);
                self.centroid = Some((Arc::clone(read), centroid));
                centroid
            }
        }
    }
}

/// What a press at `at` takes of `arrows`, the open `dialog`'s: none off every arrow.
fn took(arrows: &Arrows, dialog: &Dialog, at: Pos2) -> Option<Took> {
    match arrows.hit(at)? {
        Hit::Distance(part) => {
            let distance = arrows
                .distances
                .iter()
                .find(|distance| distance.distance.part == part)?;
            let was = dialog.inputs.iter().find_map(|input| {
                match (&input.value, input.part == part) {
                    (Value::Length(text), true) => Some(text.clone()),
                    _ => None,
                }
            })?;
            Some(Took::Distance {
                distance: distance.distance,
                arrow: distance.arrow,
                was,
            })
        }
        Hit::Flip => arrows.flip.map(Took::Flip),
    }
}

#[cfg(test)]
mod tests;
