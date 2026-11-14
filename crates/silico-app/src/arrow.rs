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
//! So a drag past the profile stops at 0.5 mm and never turns the sweep over, and a move square
//! across the arrow moves the value by nothing but still puts it on the half millimetres, as
//! the prototype's does. Nothing caps a drag above: a value past what a sweep takes is the
//! core's to refuse, as a typed one is. An arrow whose sweep the view foreshortens to under
//! 0.6 points a millimetre (seen end on, or zoomed far out) isn't drawn and takes no press, as
//! the prototype's `flat` one.
//!
//! The prototype draws a pad's alone: its pocket goes through all, with no distance. Where it's
//! silent, SolidWorks' extrude handles are followed (docs/ux/solidworks.md, §5: "dragging a
//! handle shows a dynamic preview"; Fusion gives every distance input one): a pocket's length
//! has its arrow too, along the pocket's way, against the sketch's normal; a symmetric extent's
//! one arrow stands at half its length, its value moving by twice its end's move; a two-sided
//! extent has one for each length, the second's against the sweep. Through all has none.
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
//! never queue. Esc ends a drag where it began (section 3's chain, an open drag first:
//! `Tab::escape_first`), as the stream's cancel does; a dialog closed mid-drag leaves the rest
//! of its stream doing nothing.
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
use silico_core::document::Length;
use silico_core::kernel::SketchPlane;
use silico_protocol::Camera;
use silico_render::render::project;

use crate::dialog::{self, Dialog, Kind, Part, Value};
use crate::sketch::draw::{Geometry, Scale, bounds, walk};
use crate::theme;
use crate::viewport::{LeftEvent, LeftKind};

/// How far along the sweep an arrow's way on the screen is measured, in mm
/// (`base.clone().addScaledVector(dir, 10)`).
pub const PROBE_MM: f64 = 10.0;

/// The fewest points a millimetre along the sweep an arrow is drawn at (`ppu < 0.6` is
/// `flat`).
pub const FLAT: f64 = 0.6;

/// Half a millimetre, in nanometres: a drag's value is a whole number of them, and at least
/// one (`Math.max(0.5, Math.round(v * 2) / 2)`).
pub const HALF_MM_NM: i64 = 500_000;

/// The scale and chord tolerance a profile's centroid is worked out at: a thousandth of a
/// millimetre a point, its arcs' chords within a point of them, a micrometre (at most
/// `MAX_CURVE_SEGMENTS` a curve).
const CENTROID_SCALE: Scale = Scale(1e-3);
const CENTROID_TOLERANCE: f32 = 1.0;

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
    pub factor: f64,
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
    /// A distance arrow for each length the dialog shows that holds a length above 0.
    pub distances: Vec<Distance>,
    /// Whether the flip arrow shows: while the dialog's Reversed does.
    pub flip: bool,
}

/// The centroid of `read`'s profile, in the sketch's millimetres: of every region's area, its
/// outer loop's less its holes', each loop walked as its outline is drawn ([`walk`], its arcs
/// and circles in chords within a micrometre of them). None for a sketch that makes no profile,
/// a loop that doesn't run end to end, or a profile whose area isn't above 0.
pub fn profile_centroid(read: &SketchRead) -> Option<[f64; 2]> {
    let geometry = Geometry::of(read);
    let mut rings: Vec<(Vec<[f64; 2]>, f64)> = Vec::new();
    for region in &read.regions {
        let outer = walk(&region.outer, &geometry, CENTROID_SCALE, CENTROID_TOLERANCE)?;
        rings.push((outer, 1.0));
        for hole in &region.holes {
            let hole = walk(hole, &geometry, CENTROID_SCALE, CENTROID_TOLERANCE)?;
            rings.push((hole, -1.0));
        }
    }
    // About the middle of their bounds, so a profile far from the sketch's origin keeps the
    // precision its size has.
    let (lo, hi) = bounds(rings.iter().flat_map(|(ring, _)| ring.iter().copied()))?;
    let middle = [lo[0] + (hi[0] - lo[0]) / 2.0, lo[1] + (hi[1] - lo[1]) / 2.0];
    let (mut area, mut moment) = (0.0, [0.0, 0.0]);
    for (ring, role) in &rings {
        let (ring_area, ring_moment) = moments(ring, middle);
        // However a ring runs (a lone circle runs counter-clockwise, a hole or not), it counts
        // by what it is: an outer loop's area added, a hole's taken away.
        let sense = role * ring_area.signum();
        area += sense * ring_area;
        moment = [
            moment[0] + sense * ring_moment[0],
            moment[1] + sense * ring_moment[1],
        ];
    }
    let centroid = [middle[0] + moment[0] / area, middle[1] + moment[1] / area];
    (area > 0.0 && centroid.iter().all(|c| c.is_finite())).then_some(centroid)
}

/// The polygon `ring`'s signed area (above 0 counter-clockwise) and its first moments, its
/// area times its centroid, each about `about`: the shoelace's sums.
fn moments(ring: &[[f64; 2]], about: [f64; 2]) -> (f64, [f64; 2]) {
    let (mut twice, mut x6, mut y6) = (0.0, 0.0, 0.0);
    for (at, p) in ring.iter().enumerate() {
        let q = ring[(at + 1) % ring.len()];
        let (p, q) = (
            [p[0] - about[0], p[1] - about[1]],
            [q[0] - about[0], q[1] - about[1]],
        );
        let cross = p[0] * q[1] - q[0] * p[1];
        twice += cross;
        x6 += (p[0] + q[0]) * cross;
        y6 += (p[1] + q[1]) * cross;
    }
    (twice / 2.0, [x6 / 6.0, y6 / 6.0])
}

/// `dialog`'s arrows in model space, its sketch on `plane` with its profile's centroid at
/// `centroid`: a pad's and a pocket's, as the module says; none for another kind, or a dialog
/// with no extent. A length that doesn't read as one above 0 has no arrow.
pub fn handles(dialog: &Dialog, plane: SketchPlane, centroid: [f64; 2]) -> Option<Handles> {
    let along = match dialog.kind {
        Kind::Pad => 1.0,
        Kind::Pocket => -1.0,
        _ => return None,
    };
    let form = dialog
        .inputs
        .iter()
        .find_map(|input| match (&input.part, &input.value) {
            (Part::ExtentType, Value::Choice { options, chosen }) => {
                options.get(*chosen).map(|(form, _)| form.as_str())
            }
            _ => None,
        })?;
    let reversed = dialog
        .inputs
        .iter()
        .find(|input| input.part == Part::Reversed && dialog.shown(input))
        .map(|input| matches!(input.value, Value::Toggle(true)));
    let sense = if reversed == Some(true) {
        -along
    } else {
        along
    };
    let direction = plane.normal().map(|c| c * sense);
    let start = plane.place(centroid);
    if !finite3(start) {
        return None;
    }
    let distances = dialog
        .inputs
        .iter()
        .filter(|input| dialog.shown(input))
        .filter_map(|input| {
            let (Part::ExtentNumber(field), Value::Length(text)) = (input.part, &input.value)
            else {
                return None;
            };
            let value = dialog::number(text)
                .and_then(|number| Length::parse(&number).ok())
                .filter(|value| value.nm() > 0)?;
            let (side, factor) = match (form, field) {
                ("one_side" | "two_sides", "length") => (1.0, 1.0),
                ("two_sides", "length2") => (-1.0, 1.0),
                ("symmetric", "length") => (1.0, 2.0),
                _ => return None,
            };
            let reach = value.mm() / factor;
            let way = direction.map(|c| c * side);
            let base: [f64; 3] = std::array::from_fn(|k| start[k] + reach * way[k]);
            finite3(base).then_some(Distance {
                part: input.part,
                value,
                base,
                way,
                factor,
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
    pub per_mm: f64,
    /// Its shaft's end, its length past its base.
    pub tip: Pos2,
    /// Its head: its point, [`theme::ARROW_HEAD_LENGTH`] past the shaft's end, then its
    /// corners, [`theme::ARROW_HEAD_HALF_WIDTH`] either side of the shaft's end (the left of
    /// its way first, the screen's y down).
    pub head: [Pos2; 3],
}

impl Arrow {
    /// The arrow from `base` along `unit` on the screen (a unit vector), `per_mm` points a
    /// millimetre, its shaft `length` points; none where a place of it isn't finite.
    fn new(base: [f64; 2], unit: [f64; 2], per_mm: f64, length: f32) -> Option<Arrow> {
        let along = |from: [f64; 2], by: f32| {
            [
                from[0] + unit[0] * f64::from(by),
                from[1] + unit[1] * f64::from(by),
            ]
        };
        let tip = along(base, length);
        let across = [-unit[1], unit[0]];
        let side = |k: f64| {
            let by = f64::from(theme::ARROW_HEAD_HALF_WIDTH) * k;
            point([tip[0] + across[0] * by, tip[1] + across[1] * by])
        };
        Some(Arrow {
            base: point(base)?,
            unit,
            per_mm,
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
        libm::hypot(at[0] - tip[0], at[1] - tip[1]) < f64::from(theme::ARROW_HIT_TIP)
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

/// How far `p` is from the segment from `a` to `b` (`segD`): from its nearest point on it.
fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let squared = d[0] * d[0] + d[1] * d[1];
    let t = if squared > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    libm::hypot(p[0] - a[0] - t * d[0], p[1] - a[1] - t * d[1])
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
        let p = [f64::from(origin.x) + x / ppp, f64::from(origin.y) + y / ppp];
        (ppp > 0.0 && p.iter().all(|c| c.is_finite())).then_some(p)
    };
    let arrow = |base: [f64; 3], way: [f64; 3], length: f32| -> Option<Arrow> {
        let a = on_screen(base)?;
        let b = on_screen(std::array::from_fn(|k| base[k] + PROBE_MM * way[k]))?;
        let d = [b[0] - a[0], b[1] - a[1]];
        let per_mm = libm::hypot(d[0], d[1]) / PROBE_MM;
        if !(per_mm >= FLAT && per_mm.is_finite()) {
            return None;
        }
        let unit = [d[0] / (per_mm * PROBE_MM), d[1] / (per_mm * PROBE_MM)];
        Arrow::new(a, unit, per_mm, length)
    };
    let distances = handles
        .distances
        .iter()
        .filter_map(|distance| {
            let arrow = arrow(distance.base, distance.way, theme::ARROW_LENGTH)?;
            let past = f64::from(theme::ARROW_LABEL_PAST);
            let label_at = point([
                f64::from(arrow.tip.x) + arrow.unit[0] * past + f64::from(theme::ARROW_LABEL_SHIFT),
                f64::from(arrow.tip.y) + arrow.unit[1] * past,
            ])?;
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
    let nm_per_mm = 10_i128.pow(silico_core::document::LENGTH_DECIMALS);
    format!(
        "{} mm",
        crate::properties::trimmed(i128::from(value.nm()), nm_per_mm)
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
/// on the screen over its points a millimetre is how far the end moved, in mm; the value at
/// the press moved by that times the arrow's factor, to the nearest half millimetre (a half up,
/// as `Math.round`), and at least half a millimetre. None where that isn't a length (a move
/// past what one holds).
pub fn dragged(distance: &Distance, arrow: &Arrow, press: Pos2, at: Pos2) -> Option<Length> {
    let (at, press) = (wide(at), wide(press));
    let moved =
        ((at[0] - press[0]) * arrow.unit[0] + (at[1] - press[1]) * arrow.unit[1]) / arrow.per_mm;
    let halves = round_half_up((distance.value.mm() + distance.factor * moved) * 2.0);
    if !halves.is_finite() || halves > (i64::MAX / HALF_MM_NM) as f64 {
        return None;
    }
    Some(Length::from_nm(halves.max(1.0) as i64 * HALF_MM_NM))
}

/// `x` rounded to a whole number as JavaScript's `Math.round` rounds it: to the nearest, a half
/// up, toward +∞ (`x − ⌊x⌋` is exact, so a half is found exactly).
fn round_half_up(x: f64) -> f64 {
    let below = x.floor();
    if x - below >= 0.5 { below + 1.0 } else { below }
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
                    self.held = took(arrows, dialog, at).map(|took| Held {
                        serial: dialog.map_or(0, |dialog| dialog.serial),
                        press: at,
                        press_px: event.at_px,
                        took,
                        live: true,
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

/// What a press at `at` takes of `arrows`, the open `dialog`'s: none with no dialog, or off
/// every arrow.
fn took(arrows: Option<&Arrows>, dialog: Option<&Dialog>, at: Pos2) -> Option<Took> {
    let (arrows, dialog) = (arrows?, dialog?);
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
