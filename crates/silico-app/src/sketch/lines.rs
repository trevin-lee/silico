//! Each shown sketch's curves drawn in the 3D viewport among the bodies, as the approved
//! prototype's `syncSketchLines` draws them (docs/design/m5-ui, js/app.js:968–986): each
//! curve a polyline in the sketch's plane, placed in 3D by the plane (`SketchPlane::place`),
//! in the sketch's one colour, `theme::VIEWPORT_SKETCH_WIDTH` wide, given the renderer as a
//! frame's lines (`silico_render::render::Frame::lines`): depth-tested among the bodies,
//! picked by nothing. Made from the sketch's read, which the tab keeps whole
//! (`crate::sketch::SketchReads`), never asking for one.
//!
//! - **The polylines** are sketch mode's own (`draw::polyline`), each curve at the segments
//!   its outline takes at the view's scale bucketed to the power of two at or below it
//!   (`draw::fill_scale`), its chords within `SKETCH_CHORD_TOLERANCE`. The view's scale is
//!   its millimetres a point at the sketch: an orthographic view's one value, a perspective
//!   view's at the sketch's box's point nearest the eye ([`scale_at`]), where its curves are
//!   largest on screen. A sketch's strips are made again only when its read is replaced or
//!   its bucket moves, never each frame ([`ShownLines`]).
//! - **Construction** curves are dashed in millimetres at the bucket, sketch mode's dash and
//!   gap (`SKETCH_CONSTRUCTION_DASH`, points), one strip a dash ([`dashes`]).
//! - **The colour** is the prototype's two ([`colour`]): under defined `#7a95ff`, any other
//!   state `#eeeeed`. SEAM: over defined, no solution, and invalid are `#eeeeed` too, as the
//!   prototype draws them (app.js:978), though DESIGN.md gives over defined red and invalid
//!   yellow; the port of the sketch colours decides.
//! - **None** is drawn for a read whose plane is an error (a face gone, not planar, found by
//!   an unstable name, or its feature unbuilt: nothing guesses a placement), and none for a
//!   sketch past [`MAX_LINE_POINTS`], which a warning names. Nor for a sketch whose lines
//!   would take a frame's past the renderer's most (`MAX_FRAME_LINE_POINTS`, over every
//!   shown sketch, in the order they're given), which a warning names once: the rest are
//!   drawn, so a frame is never refused for its lines' count.
//! - SEAM: Fit and Home fit the bodies alone (the core's named views, a core change); the
//!   camera's clamp takes the lines' boxes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, FeatureId};
use silico_core::sketch_states::SketchState;
use silico_protocol::vec3::{dot, norm, sub};
use silico_protocol::{Camera, Projection};
use silico_render::render::{MAX_FRAME_LINE_POINTS, Strips};

use super::draw::{Curve, Geometry, Scale, curve_segments, fill_scale, polyline};
use super::style::SKETCH_CHORD_TOLERANCE;
use crate::theme;

/// The most points one sketch's lines may have over its strips: past it the sketch isn't
/// drawn, and a warning says which, as a region's fill past `MAX_FILL_VERTICES` is left out.
pub const MAX_LINE_POINTS: usize = 200_000;

// A sketch within its own cap always fits a frame's lines alone.
const _: () = assert!(MAX_LINE_POINTS <= MAX_FRAME_LINE_POINTS);

/// The colour a sketch in `state` is drawn in, sRGB and opaque, as the prototype draws it
/// (app.js:978): under defined `#7a95ff`, any other state `#eeeeed` (the module's SEAM).
pub fn colour(state: SketchState) -> [u8; 4] {
    let colour = match state {
        SketchState::UnderDefined => theme::SKETCH_UNDER_DEFINED,
        SketchState::FullyDefined
        | SketchState::OverDefined
        | SketchState::NoSolution
        | SketchState::Invalid => theme::SKETCH_FULLY_DEFINED,
    };
    colour.to_array()
}

/// How near the eye a perspective view's scale is taken at most, as a part of the eye's
/// distance to its target: a sketch reaching nearer (past the eye, say, a plane the camera
/// is zoomed into) takes the scale there, so its curves nearest the eye may show their
/// chords, but its segments stay bounded.
const NEAREST_SCALE_DEPTH: f64 = 1.0 / 1024.0;

/// The view's millimetres a point at a sketch whose box is `bounds`, for `camera` over a
/// pane `height` points high: an orthographic view's height over the pane's; a perspective
/// view's at the box's corner nearest the eye along its view (no nearer than
/// [`NEAREST_SCALE_DEPTH`] of the eye's distance to its target), where every curve of the
/// sketch is largest on screen, so each one's chords are within the tolerance wherever it
/// lies. None for a pane of no height, a camera with no view, or a box wholly behind the
/// eye, which a perspective view doesn't draw.
pub fn scale_at(camera: &Camera, height: f32, bounds: ([f64; 3], [f64; 3])) -> Option<Scale> {
    let height = f64::from(height);
    if height.is_nan() || height <= 0.0 {
        return None;
    }
    let scale = match camera.projection {
        Projection::Orthographic { view_height } => view_height / height,
        Projection::Perspective { field_of_view } => {
            let towards = sub(camera.target, camera.eye);
            let focus = norm(towards);
            if focus.is_nan() || focus <= 0.0 {
                return None;
            }
            let view = towards.map(|c| c / focus);
            let (low, high) = bounds;
            let (nearest, farthest) = (0..8u8)
                .map(|corner| {
                    let at: [f64; 3] = std::array::from_fn(|k| {
                        if corner >> k & 1 == 0 {
                            low[k]
                        } else {
                            high[k]
                        }
                    });
                    dot(sub(at, camera.eye), view)
                })
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(near, far), depth| {
                    (near.min(depth), far.max(depth))
                });
            if farthest.is_nan() || farthest <= 0.0 {
                return None;
            }
            let depth = nearest.max(focus * NEAREST_SCALE_DEPTH);
            2.0 * depth * libm::tan(field_of_view.to_radians() / 2.0) / height
        }
    };
    (scale > 0.0 && scale.is_finite()).then_some(Scale(scale))
}

/// Why a sketch's lines aren't drawn though its plane places it.
#[derive(Debug, Clone, PartialEq)]
pub enum NotDrawn {
    /// More points than [`MAX_LINE_POINTS`] at the bucket's scale.
    TooMany,
    /// The renderer refused them (a point past `MAX_RENDER_COORDINATE`).
    Refused(String),
}

/// `read`'s curves as strips in the document's coordinates at `scale` (a bucket's,
/// [`fill_scale`]'s): each curve sketch mode's polyline, construction dashed, each point
/// placed by the read's plane. None for a read its plane doesn't place.
pub fn strips_of(read: &SketchRead, scale: Scale) -> Option<Result<Strips, NotDrawn>> {
    let plane = *read.plane.as_ref().ok()?;
    let construction = construction_of(read);
    let [dash, gap] = theme::SKETCH_CONSTRUCTION_DASH.map(|points| scale.mm(points));
    let mut polylines: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut points = 0usize;
    for (id, curve, _) in Geometry::of(read).curves() {
        let line = polyline(curve, scale, SKETCH_CHORD_TOLERANCE);
        let pieces = if construction.get(id).copied().unwrap_or(false) {
            dashes(&line, dash, gap)
        } else {
            vec![line]
        };
        for piece in pieces {
            points += piece.len();
            if points > MAX_LINE_POINTS {
                return Some(Err(NotDrawn::TooMany));
            }
            polylines.push(piece);
        }
    }
    let placed = polylines
        .into_iter()
        .filter(|piece| piece.len() >= 2)
        .map(|piece| piece.into_iter().map(|at| plane.place(at)).collect());
    Some(Strips::new(placed).map_err(|refused| NotDrawn::Refused(refused.message)))
}

/// Whether each of `read`'s curves is construction, by its ID.
fn construction_of(read: &SketchRead) -> BTreeMap<&ElementId, bool> {
    let members = &read.members;
    members
        .lines
        .iter()
        .map(|l| (&l.id, l.construction))
        .chain(members.circles.iter().map(|c| (&c.id, c.construction)))
        .chain(members.arcs.iter().map(|a| (&a.id, a.construction)))
        .collect()
}

/// How many points [`strips_of`] makes of `read` at `scale`, counted without making any
/// polyline: a line's 2, a circle's or an arc's its segments and 1 ([`curve_segments`], as
/// [`polyline`] takes them). A construction curve's, at most: its vertices, each in one
/// dash at most, and each dash's two ends, the dashes no more than its length (a line's, or
/// an arc's r·|sweep|, never less than its polyline's) over a dash and a gap, rounded up, nor
/// than [`MAX_CURVE_DASHES`] and 1. So it's never below what's made, and the caps hold; a
/// sketch of construction curves can be counted past what's left, or its own cap, though
/// what it would make fits. Past [`MAX_LINE_POINTS`] it stops, at the first count over it.
/// None for a read its plane doesn't place.
fn count_points(read: &SketchRead, scale: Scale) -> Option<usize> {
    read.plane.as_ref().ok()?;
    let construction = construction_of(read);
    let [dash, gap] = theme::SKETCH_CONSTRUCTION_DASH.map(|points| scale.mm(points));
    let most_dashes = MAX_CURVE_DASHES + 1;
    let mut points = 0usize;
    for (id, curve, _) in Geometry::of(read).curves() {
        let (vertices, length) = match curve {
            Curve::Line { from, to } => (2, libm::hypot(to[0] - from[0], to[1] - from[1])),
            Curve::Round { radius, sweep, .. } => (
                curve_segments(radius, sweep, scale, SKETCH_CHORD_TOLERANCE) + 1,
                radius * sweep.abs(),
            ),
        };
        points += if construction.get(id).copied().unwrap_or(false) {
            let dashes = (length / (dash + gap)).ceil();
            let dashes = if dashes >= 0.0 && dashes < most_dashes as f64 {
                dashes as usize
            } else {
                most_dashes
            };
            vertices + 2 * dashes
        } else {
            vertices
        };
        if points > MAX_LINE_POINTS {
            break;
        }
    }
    Some(points)
}

/// The most points of the shown sketches' lines a frame builds: a quarter of what it may
/// draw, so lines made again after a zoom are spread over a few frames.
pub const MAX_FRAME_LINE_BUILD: usize = MAX_FRAME_LINE_POINTS / 4;

// A sketch within its own cap is always built in a frame of its own.
const _: () = assert!(MAX_LINE_POINTS <= MAX_FRAME_LINE_BUILD);

/// The points a frame may draw of the shown sketches' lines, and may build of them (any
/// made again after a read or a bucket changed): what [`ShownLines::frame`] gives each
/// frame, the renderer's most ([`MAX_FRAME_LINE_POINTS`]) and [`MAX_FRAME_LINE_BUILD`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Budget {
    draw: usize,
    build: usize,
}

/// A shown sketch's lines as the frame gives them the renderer: its strips, shared with
/// the cache that made them, and its colour; its width is the theme's.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameLines {
    pub strips: Arc<Strips>,
    pub colour: [u8; 4],
}

/// What was made of a sketch's read: the read (so a replaced one is seen by its `Arc`), its
/// box ([`box_of`], none for one its plane doesn't place), how many points it takes at a
/// bucket ([`count_points`]), and, at the bucket they were made at, the strips or why there
/// are none.
struct Made {
    read: Arc<SketchRead>,
    bounds: Option<([f64; 3], [f64; 3])>,
    count: Option<(i32, Option<usize>)>,
    strips: Option<(i32, Result<Arc<Strips>, NotDrawn>)>,
}

/// Each shown sketch's lines, kept from frame to frame: a sketch's made again only when
/// its read is replaced or the view's scale at it leaves its bucket, within each frame's
/// budget ([`Budget`]), the rest made in the frames after.
#[derive(Default)]
pub struct ShownLines {
    made: BTreeMap<FeatureId, Made>,
    /// The sketches the last frame left out for its lines' most points, so each is warned
    /// of once while it's left out, not each frame.
    capped: BTreeSet<FeatureId>,
    /// Whether the last frame left a sketch's lines to make for its build budget: a frame
    /// to come makes them, asked for by the viewport ([`pending`](Self::pending)).
    pending: bool,
    /// The points the last frame built: what a test counts.
    built: usize,
}

/// A read's box in the document's coordinates, worked out from its members alone (each
/// point, and each circle's and arc's square about its centre), placed by its plane: what
/// the view's scale at it is taken at, before any strip is made.
fn box_of(read: &SketchRead) -> Option<([f64; 3], [f64; 3])> {
    let plane = *read.plane.as_ref().ok()?;
    let geometry = Geometry::of(read);
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    let mut take = |[x, y]: [f64; 2], r: f64| {
        low = [low[0].min(x - r), low[1].min(y - r)];
        high = [high[0].max(x + r), high[1].max(y + r)];
    };
    for (_, at) in geometry.points() {
        take(at, 0.0);
    }
    for (_, curve, _) in geometry.curves() {
        if let Curve::Round { centre, radius, .. } = curve {
            take(centre, radius);
        }
    }
    if !(low[0] <= high[0] && low[1] <= high[1]) {
        return None;
    }
    let corners = [
        [low[0], low[1]],
        [high[0], low[1]],
        [low[0], high[1]],
        [high[0], high[1]],
    ]
    .map(|at| plane.place(at));
    let lo = std::array::from_fn(|k| corners.iter().map(|c| c[k]).fold(f64::INFINITY, f64::min));
    let hi = std::array::from_fn(|k| {
        corners
            .iter()
            .map(|c| c[k])
            .fold(f64::NEG_INFINITY, f64::max)
    });
    Some((lo, hi))
}

impl ShownLines {
    /// The lines of `shown` (each shown sketch's read, by its ID), at the view's scale for
    /// `camera` over a pane `height` points high, each made again only when its read was
    /// replaced or its bucket moved; a sketch no longer shown let go. Its boxes too, for the
    /// camera's clamp. At most [`MAX_FRAME_LINE_POINTS`] drawn, and as many built
    /// ([`Self::frame_within`]).
    pub fn frame<'a>(
        &mut self,
        shown: impl IntoIterator<Item = (&'a FeatureId, &'a Arc<SketchRead>)>,
        camera: &Camera,
        height: f32,
    ) -> Vec<FrameLines> {
        let budget = Budget {
            draw: MAX_FRAME_LINE_POINTS,
            build: MAX_FRAME_LINE_BUILD,
        };
        self.frame_within(shown, camera, height, budget)
    }

    /// Whether the last frame left lines to make for its build budget: the next frame makes
    /// more of them, drawing each sketch's old lines (its last bucket's) till then.
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// [`Self::frame`] within `budget`. Each sketch, in `shown`'s order: made again if its
    /// read or bucket changed, within what's left of both budgets (never past what's left
    /// to draw, so a sketch the frame can't draw keeps nothing); drawn if its points fit in
    /// what the ones before it left, and else left out, its strips let go and a warning
    /// naming it the first frame it is. One whose build is past what's left to build keeps
    /// its old lines, drawn if they fit, and is made in a frame to come.
    fn frame_within<'a>(
        &mut self,
        shown: impl IntoIterator<Item = (&'a FeatureId, &'a Arc<SketchRead>)>,
        camera: &Camera,
        height: f32,
        budget: Budget,
    ) -> Vec<FrameLines> {
        let mut kept = BTreeMap::new();
        let mut lines = Vec::new();
        let mut left = budget.draw;
        let mut build_left = budget.build;
        let mut capped = BTreeSet::new();
        self.pending = false;
        self.built = 0;
        for (id, read) in shown {
            // The box worked out once a read; the strips once a read and a bucket.
            let mut made = match self.made.remove(id) {
                Some(made) if Arc::ptr_eq(&made.read, read) => made,
                _ => Made {
                    read: Arc::clone(read),
                    bounds: box_of(read),
                    count: None,
                    strips: None,
                },
            };
            let scale = made
                .bounds
                .and_then(|bounds| scale_at(camera, height, bounds));
            // Past what's left to draw: left out, whatever its lines would be.
            let mut out = false;
            if let Some(scale) = scale {
                let (bucket, at) = fill_scale(scale);
                if made
                    .strips
                    .as_ref()
                    .is_none_or(|(made_at, _)| *made_at != bucket)
                {
                    // Counted once a read and a bucket, before anything is built.
                    let count = match made.count {
                        Some((counted_at, count)) if counted_at == bucket => count,
                        _ => {
                            let count = count_points(read, at);
                            made.count = Some((bucket, count));
                            count
                        }
                    };
                    match count {
                        None => made.strips = None,
                        Some(count) if count > MAX_LINE_POINTS => {
                            let why = NotDrawn::TooMany;
                            tracing::warn!(sketch = %id, ?why, "a shown sketch's lines aren't drawn");
                            made.strips = Some((bucket, Err(why)));
                        }
                        // Past what's left to draw: left out, nothing built or spent.
                        Some(count) if count > left => {
                            out = true;
                            made.strips = None;
                        }
                        // Past what's left to build, a frame's build begun: made in a frame
                        // to come, its old lines drawn meanwhile, if they fit.
                        Some(count) if count > build_left && build_left < budget.build => {
                            self.pending = true;
                        }
                        // Charged its count whatever the build comes to, refused or not.
                        Some(count) => {
                            let built = strips_of(read, at)
                                .unwrap_or(Err(NotDrawn::TooMany))
                                .map(Arc::new);
                            build_left -= count.min(build_left);
                            self.built += count;
                            if let Err(why) = &built {
                                tracing::warn!(sketch = %id, ?why, "a shown sketch's lines aren't drawn");
                            }
                            made.strips = Some((bucket, built));
                        }
                    }
                }
            }
            let fits = match &made.strips {
                Some((_, Ok(strips))) if !strips.is_empty() && !out => {
                    if strips.points() <= left {
                        left -= strips.points();
                        lines.push(FrameLines {
                            strips: Arc::clone(strips),
                            colour: colour(read.states.state),
                        });
                        true
                    } else {
                        false
                    }
                }
                _ => !out,
            };
            if !fits {
                if !self.capped.contains(id) {
                    tracing::warn!(
                        sketch = %id,
                        left,
                        "a shown sketch's lines aren't drawn: past the frame's most points"
                    );
                }
                capped.insert(id.clone());
                made.strips = None;
            }
            kept.insert(id.clone(), made);
        }
        self.made = kept;
        self.capped = capped;
        lines
    }
}

/// The most dashes one curve is cut into: past it its dashes and gaps are lengthened alike
/// to fit, so a long construction curve seen closely costs no more than its outline.
pub const MAX_CURVE_DASHES: usize = super::draw::MAX_CURVE_SEGMENTS;

/// `polyline` cut into dashes `on` long with gaps `off` long between them, measured along
/// it from its start, each dash a polyline of its own (its ends on the polyline, and every
/// vertex between them): how a construction curve is drawn. Where that would make more than
/// [`MAX_CURVE_DASHES`], both are lengthened by one factor to make that many (however many
/// periods the line holds, past f64's range too). A dash and gap whose sum overflows f64 are
/// a period longer than the line: one dash, from its start. A polyline of no length, or a
/// dash or gap that isn't a finite length above 0 (the dash) or at least 0 (the gap), makes
/// none.
pub fn dashes(polyline: &[[f64; 2]], on: f64, off: f64) -> Vec<Vec<[f64; 2]>> {
    let mut dashes = Vec::new();
    if !(on.is_finite() && off.is_finite() && on > 0.0 && off >= 0.0) {
        return dashes;
    }
    // How far along the polyline each of its points is.
    let mut along = Vec::with_capacity(polyline.len());
    let mut total = 0.0;
    for (k, point) in polyline.iter().enumerate() {
        if let Some(before) = k.checked_sub(1).and_then(|k| polyline.get(k)) {
            total += libm::hypot(point[0] - before[0], point[1] - before[1]);
        }
        along.push(total);
    }
    if !(total > 0.0 && total.is_finite()) {
        return dashes;
    }
    let most = MAX_CURVE_DASHES as f64;
    // A period at least the line's length makes one dash, so one past f64's range (whose sum
    // overflows) is taken as the line's length: the same dashes, with a period of a number.
    let period = (on + off).min(total);
    let stretch = total / period / most;
    let (on, period) = if stretch.is_infinite() {
        // More periods than f64 counts (a tiny period on a long line): `most` of them, each
        // the line's length over `most`, its dash the same share of it as before.
        let period_stretched = total / most;
        (period_stretched * (on / period), period_stretched)
    } else {
        let stretch = stretch.max(1.0);
        (on * stretch, period * stretch)
    };
    // The point `s` along, on the segment from point `k` (whose length along is at most
    // `s`, and the next's at least).
    let at = |k: usize, s: f64| -> [f64; 2] {
        let next = (k + 1).min(polyline.len() - 1);
        let (a, b) = (polyline[k], polyline[next]);
        let length = along[next] - along[k];
        let t = if length > 0.0 {
            ((s - along[k]) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    };
    let mut k = 0usize;
    // Each dash's start a whole number of periods along, never a sum of them (whole numbers
    // are exact in f64 far past `MAX_CURVE_DASHES`).
    let mut n = 0.0_f64;
    while n < most + 1.0 {
        let start = n * period;
        n += 1.0;
        if start.is_nan() || total.is_nan() || start >= total {
            break;
        }
        let end = (start + on).min(total);
        while k + 1 < along.len() && along[k + 1] <= start {
            k += 1;
        }
        let mut dash = vec![at(k, start)];
        let mut j = k + 1;
        while j < along.len() && along[j] < end {
            dash.push(polyline[j]);
            j += 1;
        }
        dash.push(at(j - 1, end));
        dashes.push(dash);
    }
    dashes
}

#[cfg(test)]
mod tests {
    use silico_core::document::{Coordinate, OriginPlane, SketchCircle};

    use super::super::tests::{id, line, point, rectangle, under};
    use super::*;

    /// The study's test 10's Loose: a 40 by 20 rectangle on XZ from the origin, no
    /// constraints, so under defined.
    fn loose() -> SketchRead {
        SketchRead {
            plane: Ok(OriginPlane::Xz.into()),
            ..under(rectangle(Vec::new()))
        }
    }

    #[test]
    fn a_rectangle_on_xz_is_four_strips_in_its_lines_order_placed_in_the_plane() {
        // The study's test 10: each side from one corner to the next, (x, y) on XZ placed at
        // (x, 0, y).
        let strips = strips_of(&loose(), Scale(0.125)).unwrap().unwrap();
        assert_eq!(strips.len(), 4);
        let sides: Vec<&[[f64; 3]]> = (0..4).filter_map(|k| strips.strip(k)).collect();
        assert_eq!(
            sides,
            [
                &[[0.0, 0.0, 0.0], [40.0, 0.0, 0.0]][..],
                &[[40.0, 0.0, 0.0], [40.0, 0.0, 20.0]][..],
                &[[40.0, 0.0, 20.0], [0.0, 0.0, 20.0]][..],
                &[[0.0, 0.0, 20.0], [0.0, 0.0, 0.0]][..],
            ]
        );
        assert_eq!(colour(loose().states.state), [0x7a, 0x95, 0xff, 0xff]);
    }

    #[test]
    fn a_read_its_plane_doesnt_place_makes_no_strips() {
        let mut read = loose();
        read.plane = Err(silico_core::Error::new(
            silico_core::ErrorCode::ReferenceBroken,
            "the face is gone",
        ));
        assert!(strips_of(&read, Scale(0.125)).is_none());
    }

    #[test]
    fn a_small_circle_takes_twenty_one_segments_at_its_bucket_within_a_quarter_point() {
        // The study's hand value: radius 2.75 at (40, 12) on XY, 120 mm over 800 points
        // (0.15 mm a point) buckets to 0.125; 22 points (21 segments), the first and last
        // (42.75, 12, 0), each 2.75 from the centre to 1e-12, no chord's sagitta past
        // 0.03125 mm (a quarter point at the bucket).
        let mut sketch = rectangle(Vec::new());
        sketch.lines.clear();
        sketch.points = vec![point("pa000006", 40.0, 12.0)];
        sketch.circles.push(SketchCircle {
            id: id("ca000001"),
            center: id("pa000006"),
            radius: Coordinate::new(2.75).unwrap(),
            construction: false,
        });
        let (bucket, at) = fill_scale(Scale(120.0 / 800.0));
        assert_eq!((bucket, at), (-3, Scale(0.125)));
        let strips = strips_of(&under(sketch), at).unwrap().unwrap();
        assert_eq!((strips.len(), strips.points()), (1, 22));
        let points = strips.strip(0).unwrap();
        assert_eq!(points[0], [42.75, 12.0, 0.0]);
        let last = points[21];
        assert!((last[0] - 42.75).abs() < 1e-12 && (last[1] - 12.0).abs() < 1e-12);
        for point in points {
            let off = libm::hypot(point[0] - 40.0, point[1] - 12.0);
            assert!((off - 2.75).abs() < 1e-12, "{point:?}");
        }
        // Each chord's sagitta, 2.75 (1 − cos(π/21)), within a quarter point at the bucket.
        let sagitta = 2.75 * (1.0 - libm::cos(std::f64::consts::PI / 21.0));
        assert!(sagitta < 0.25 * 0.125 && sagitta > 0.0307, "{sagitta}");
        let (low, high) = strips.bounds().unwrap();
        // Its vertices at 2πk/21 from (42.75, 12): the rightmost the first, the leftmost
        // the 10th and 11th, 2.75 cos(π/21) left of the centre.
        let leftmost = 40.0 - 2.75 * libm::cos(std::f64::consts::PI / 21.0);
        assert!((high[0] - 42.75).abs() < 1e-12, "{high:?}");
        assert!((low[0] - leftmost).abs() < 1e-12, "{low:?}");
        assert_eq!((low[2], high[2]), (0.0, 0.0));
    }

    #[test]
    fn a_construction_line_is_cut_into_dashes_of_five_points_and_gaps_of_three() {
        // 40 mm at 0.125 mm a point: dashes of 0.625 mm, gaps of 0.375, a period of 1 mm; 40
        // dashes, each two points (a line has no vertex between its ends), the last at 39.
        let mut sketch = rectangle(Vec::new());
        sketch.lines.truncate(1);
        sketch.lines[0].construction = true;
        let strips = strips_of(&under(sketch), Scale(0.125)).unwrap().unwrap();
        assert_eq!((strips.len(), strips.points()), (40, 80));
        assert_eq!(strips.bounds(), Some(([0.0, 0.0, 0.0], [39.625, 0.0, 0.0])));
    }

    #[test]
    fn dashes_follow_the_polyline_round_its_corners_and_stop_at_its_end() {
        // An L, (0, 0) to (3, 0) to (3, 3): dashes of 2 and gaps of 1.
        let l = [[0.0, 0.0], [3.0, 0.0], [3.0, 3.0]];
        assert_eq!(
            dashes(&l, 2.0, 1.0),
            vec![vec![[0.0, 0.0], [2.0, 0.0]], vec![[3.0, 0.0], [3.0, 2.0]],]
        );
        // A dash over the corner keeps it: dashes of 2.5, no gap.
        assert_eq!(
            dashes(&l, 2.5, 0.0),
            vec![
                vec![[0.0, 0.0], [2.5, 0.0]],
                vec![[2.5, 0.0], [3.0, 0.0], [3.0, 2.0]],
                vec![[3.0, 2.0], [3.0, 3.0]],
            ]
        );
        // A dash or gap that isn't a length, or a polyline of no length: no dashes.
        assert_eq!(dashes(&l, 0.0, 1.0), Vec::<Vec<[f64; 2]>>::new());
        assert_eq!(dashes(&l, f64::NAN, 1.0), Vec::<Vec<[f64; 2]>>::new());
        assert_eq!(dashes(&l, 1.0, -1.0), Vec::<Vec<[f64; 2]>>::new());
        assert_eq!(
            dashes(&[[1.0, 1.0], [1.0, 1.0]], 1.0, 1.0),
            Vec::<Vec<[f64; 2]>>::new()
        );
        // A line 2048 periods long: 1024 dashes, each and its gap twice as long, the last
        // from 2046 to 2047.
        let long = dashes(&[[0.0, 0.0], [2048.0, 0.0]], 0.5, 0.5);
        assert_eq!(long.len(), 1024);
        assert_eq!(long[0], vec![[0.0, 0.0], [1.0, 0.0]]);
        assert_eq!(long[1023], vec![[2046.0, 0.0], [2047.0, 0.0]]);
    }

    #[test]
    fn a_sketch_past_its_points_cap_isnt_drawn() {
        // 200 circles of 1 m at a millionth of a millimetre a point: each at its most
        // segments, 1024 (1025 points), 205,000 points in all, past the cap's 200,000; 195
        // are within it.
        let circles = |n: usize| {
            let mut sketch = rectangle(Vec::new());
            sketch.lines.clear();
            sketch.points = vec![point("pa000006", 0.0, 0.0)];
            sketch.circles = (0..n)
                .map(|k| SketchCircle {
                    id: id(&format!("c{k:07}")),
                    center: id("pa000006"),
                    radius: Coordinate::new(1000.0).unwrap(),
                    construction: false,
                })
                .collect();
            under(sketch)
        };
        assert_eq!(
            strips_of(&circles(200), Scale(1e-6)).map(|made| made.err()),
            Some(Some(NotDrawn::TooMany))
        );
        let within = strips_of(&circles(195), Scale(1e-6)).unwrap().unwrap();
        assert_eq!(within.points(), 195 * 1025);
    }

    #[test]
    fn the_scale_is_an_orthographic_views_or_a_perspectives_at_the_sketchs_nearest_corner() {
        let ortho = Camera {
            projection: Projection::Orthographic { view_height: 120.0 },
            eye: [0.0, -100.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        };
        let anywhere = ([0.0; 3], [1.0; 3]);
        assert_eq!(scale_at(&ortho, 800.0, anywhere), Some(Scale(0.15)));
        assert_eq!(scale_at(&ortho, 0.0, anywhere), None);
        // 90° vertical: at depth d the view is 2d high; a box whose nearest corner is 50 in
        // front of the eye, over 500 points: 100 / 500 = 0.2 mm a point.
        let wide = Camera {
            projection: Projection::Perspective {
                field_of_view: 90.0,
            },
            ..ortho
        };
        let near = ([-1.0, -50.0, -1.0], [1.0, 20.0, 1.0]);
        let scale = scale_at(&wide, 500.0, near).unwrap();
        assert!((scale.0 - 0.2).abs() < 1e-12, "{scale:?}");
        // Wholly behind the eye: none. Reaching past it: at a 1024th of the target's
        // distance, 100/1024, the view 2 × 100/1024 high over 500 points.
        assert_eq!(
            scale_at(&wide, 500.0, ([0.0, -300.0, 0.0], [0.0, -200.0, 0.0])),
            None
        );
        let through = scale_at(&wide, 500.0, ([0.0, -300.0, 0.0], [0.0, 0.0, 0.0])).unwrap();
        assert!(
            (through.0 - 2.0 * 100.0 / 1024.0 / 500.0).abs() < 1e-15,
            "{through:?}"
        );
    }

    #[test]
    fn a_sketch_on_a_face_ten_kilometres_out_is_placed_in_f64() {
        // Loose's rectangle on a face frame at (10⁷, 3, 0.5), x along +y, y along +z: its
        // first corner there exactly, its third 40 along y and 20 along z, no f32 rounding
        // (whose step at 10⁷ is 1 mm).
        let mut read = loose();
        read.plane = Ok(silico_core::kernel::SketchPlane::Face(
            silico_protocol::FaceFrame {
                origin: [1e7, 3.0, 0.5],
                x: [0.0, 1.0, 0.0],
                y: [0.0, 0.0, 1.0],
            },
        ));
        let strips = strips_of(&read, Scale(0.125)).unwrap().unwrap();
        assert_eq!(
            strips.strip(1),
            Some(&[[1e7, 43.0, 0.5], [1e7, 43.0, 20.5]][..])
        );
    }

    #[test]
    fn a_sketchs_lines_are_made_again_only_for_a_new_read_or_a_new_bucket() {
        // The same read's `Arc` at the same scale: the same strips (one `Arc`); a new read,
        // equal or not, or a scale in another power of two: made again. A sketch no longer
        // shown is let go.
        let camera = |view_height: f64| Camera {
            projection: Projection::Orthographic { view_height },
            eye: [0.0, -100.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        };
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let (loose_id, read) = (fid("s0000002"), Arc::new(loose()));
        let mut shown = ShownLines::default();
        let first = shown.frame([(&loose_id, &read)], &camera(120.0), 800.0);
        assert_eq!(first.len(), 1);
        let again = shown.frame([(&loose_id, &read)], &camera(110.0), 800.0);
        assert!(
            Arc::ptr_eq(&first[0].strips, &again[0].strips),
            "the same bucket"
        );
        let finer = shown.frame([(&loose_id, &read)], &camera(60.0), 800.0);
        assert!(
            !Arc::ptr_eq(&first[0].strips, &finer[0].strips),
            "a new bucket"
        );
        let replaced = Arc::new(loose());
        let new = shown.frame([(&loose_id, &replaced)], &camera(60.0), 800.0);
        assert!(!Arc::ptr_eq(&finer[0].strips, &new[0].strips), "a new read");
        assert!(shown.frame([], &camera(60.0), 800.0).is_empty());
        assert!(shown.made.is_empty());
    }

    /// 120 mm over 800 points onto the origin from −y, orthographic: bucket 0.125.
    fn at_120() -> Camera {
        Camera {
            projection: Projection::Orthographic { view_height: 120.0 },
            eye: [0.0, -100.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        }
    }

    /// A circle of radius 2.75 at (40, 12) on XY: 22 points at bucket 0.125 (the small
    /// circle's test).
    fn round() -> SketchRead {
        let mut round = rectangle(Vec::new());
        round.lines.clear();
        round.points = vec![point("pa000006", 40.0, 12.0)];
        round.circles.push(SketchCircle {
            id: id("ca000001"),
            center: id("pa000006"),
            radius: Coordinate::new(2.75).unwrap(),
            construction: false,
        });
        under(round)
    }

    /// Loose's rectangle's first side alone: one strip of 2 points.
    fn side() -> SketchRead {
        let mut side = rectangle(Vec::new());
        side.lines.truncate(1);
        under(side)
    }

    /// How many warnings `f` logs.
    fn warned(f: impl FnOnce()) -> usize {
        let warnings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        tracing::subscriber::with_default(super::super::tests::Warnings(Arc::clone(&warnings)), f);
        warnings.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn points(drawn: &[FrameLines]) -> Vec<usize> {
        drawn.iter().map(|line| line.strips.points()).collect()
    }

    #[test]
    fn a_sketch_that_would_pass_the_frames_most_points_is_left_out_and_the_rest_drawn() {
        // Shown in this order, their IDs not: s0000003, Loose's rectangle (8 points);
        // s0000001, the circle (22); s0000002, one side (2). Within 20 points to draw: the
        // rectangle, not the circle (8 + 22 = 30), the side: [8, 2]. In ID order or reversed
        // it would be [2, 8]. The circle, left out, keeps no strips, is never built, and is
        // warned of once while it stays out: three frames, one warning; drawn at 40, then out
        // again at 20, a second.
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let (rect, circle, one) = (fid("s0000003"), fid("s0000001"), fid("s0000002"));
        let reads = [Arc::new(loose()), Arc::new(round()), Arc::new(side())];
        let shown = [(&rect, &reads[0]), (&circle, &reads[1]), (&one, &reads[2])];
        let within = |draw: usize| Budget {
            draw,
            build: MAX_FRAME_LINE_POINTS,
        };
        let mut lines = ShownLines::default();
        let out = warned(|| {
            for frame in 0..3 {
                let drawn = lines.frame_within(shown, &at_120(), 800.0, within(20));
                assert_eq!(points(&drawn), [8, 2]);
                assert_eq!(lines.capped, BTreeSet::from([circle.clone()]));
                assert!(lines.made[&circle].strips.is_none(), "none kept for it");
                // The circle, counted past the 12 left, never built; the rectangle and the
                // side built the first frame, and kept.
                let built = if frame == 0 { 8 + 2 } else { 0 };
                assert_eq!(lines.built, built, "frame {frame}");
            }
        });
        assert_eq!(out, 1);
        assert_eq!(lines.made.len(), 3);
        let again = warned(|| {
            let drawn = lines.frame_within(shown, &at_120(), 800.0, within(40));
            assert_eq!(points(&drawn), [8, 22, 2]);
            assert!(lines.capped.is_empty());
            lines.frame_within(shown, &at_120(), 800.0, within(20));
        });
        assert_eq!(again, 1);
        // At the renderer's most, all three, as `frame` gives it.
        assert_eq!(points(&lines.frame(shown, &at_120(), 800.0)), [8, 22, 2]);
    }

    #[test]
    fn lines_made_again_are_built_within_each_frames_budget_the_old_drawn_meanwhile() {
        // The rectangle (8), the circle (22), the rectangle again (8), building at most 30 a
        // frame: the first frame builds 8 + 22 and leaves the third for the next, asking for
        // it; the next builds it. A new bucket (60 mm, 0.0625) with 10 a frame: the
        // rectangle rebuilt (8), the circle's and the third's old lines (bucket 0.125) drawn
        // meanwhile, the same `Arc`s, till frames to come build them (100 a frame: the
        // circle, 31 points at 0.0625, and the third).
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let (a, b, c) = (fid("s0000001"), fid("s0000002"), fid("s0000003"));
        let reads = [Arc::new(loose()), Arc::new(round()), Arc::new(loose())];
        let shown = [(&a, &reads[0]), (&b, &reads[1]), (&c, &reads[2])];
        let within = |build: usize| Budget {
            draw: MAX_FRAME_LINE_POINTS,
            build,
        };
        let mut lines = ShownLines::default();
        let first = lines.frame_within(shown, &at_120(), 800.0, within(30));
        assert_eq!(points(&first), [8, 22]);
        assert_eq!(lines.built, 30);
        assert!(lines.pending());
        let second = lines.frame_within(shown, &at_120(), 800.0, within(30));
        assert_eq!(points(&second), [8, 22, 8]);
        assert_eq!(lines.built, 8);
        assert!(!lines.pending());
        let closer = Camera {
            projection: Projection::Orthographic { view_height: 60.0 },
            ..at_120()
        };
        let third = lines.frame_within(shown, &closer, 800.0, within(10));
        assert!(lines.built <= 10, "{}", lines.built);
        assert!(lines.pending());
        assert_eq!(third.len(), 3);
        assert!(
            !Arc::ptr_eq(&third[0].strips, &second[0].strips),
            "made again"
        );
        assert!(
            Arc::ptr_eq(&third[1].strips, &second[1].strips),
            "the old drawn"
        );
        assert!(
            Arc::ptr_eq(&third[2].strips, &second[2].strips),
            "the old drawn"
        );
        let mut frames = 0;
        while lines.pending() {
            lines.frame_within(shown, &closer, 800.0, within(100));
            assert!(lines.built <= 100, "{}", lines.built);
            frames += 1;
            assert!(frames < 10);
        }
    }

    #[test]
    fn each_state_has_the_prototypes_colour() {
        // The prototype's two (app.js:978): under defined #7a95ff, every other #eeeeed.
        for (state, rgb) in [
            (SketchState::UnderDefined, 0x7a95ff),
            (SketchState::FullyDefined, 0xeeeeed),
            (SketchState::OverDefined, 0xeeeeed),
            (SketchState::NoSolution, 0xeeeeed),
            (SketchState::Invalid, 0xeeeeed),
        ] {
            let [r, g, b, a] = colour(state);
            assert_eq!(
                ((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b), a),
                (rgb, 0xff),
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_sketch_past_its_own_cap_is_warned_of_once_for_each_read_and_bucket() {
        // 200 circles of 1 m at a bucket of 2⁻²⁰ mm a point (0.000_8 mm over 800 points is
        // 10⁻⁶): 205,000 points, past 200,000. Three frames: one warning. Its read
        // replaced: one more. A new bucket: one more. None drawn.
        let circles = || {
            let mut sketch = rectangle(Vec::new());
            sketch.lines.clear();
            sketch.points = vec![point("pa000006", 0.0, 0.0)];
            sketch.circles = (0..200)
                .map(|k| SketchCircle {
                    id: id(&format!("c{k:07}")),
                    center: id("pa000006"),
                    radius: Coordinate::new(1000.0).unwrap(),
                    construction: false,
                })
                .collect();
            Arc::new(under(sketch))
        };
        let tiny = |view_height: f64| Camera {
            projection: Projection::Orthographic { view_height },
            ..at_120()
        };
        let sid = FeatureId::new("s0000001").unwrap();
        let (first, second) = (circles(), circles());
        let mut lines = ShownLines::default();
        let warnings = warned(|| {
            for _ in 0..3 {
                assert!(
                    lines
                        .frame([(&sid, &first)], &tiny(0.000_8), 800.0)
                        .is_empty()
                );
            }
        });
        assert_eq!(warnings, 1);
        assert_eq!(
            warned(|| {
                lines.frame([(&sid, &second)], &tiny(0.000_8), 800.0);
                lines.frame([(&sid, &second)], &tiny(0.000_8), 800.0);
            }),
            1
        );
        assert_eq!(
            warned(|| {
                lines.frame([(&sid, &second)], &tiny(0.000_4), 800.0);
            }),
            1
        );
    }

    #[test]
    fn a_frame_draws_and_builds_no_more_than_the_renderers_most() {
        // Eleven sketches of 195 circles of 1 m at 2⁻²⁰ mm a point, each 195 · 1,025 =
        // 199,875 points. At most 500,000 built a frame (MAX_FRAME_LINE_BUILD): two a frame,
        // the rest left to the frames after, the view asked again meanwhile. Settled: ten
        // drawn, fitting 2,000,000 (1,998,750); the eleventh, with 1,250 left, out, nothing of
        // it built or kept.
        assert_eq!(MAX_FRAME_LINE_BUILD, 500_000);
        let sketch = || {
            let mut sketch = rectangle(Vec::new());
            sketch.lines.clear();
            sketch.points = vec![point("pa000006", 0.0, 0.0)];
            sketch.circles = (0..195)
                .map(|k| SketchCircle {
                    id: id(&format!("c{k:07}")),
                    center: id("pa000006"),
                    radius: Coordinate::new(1000.0).unwrap(),
                    construction: false,
                })
                .collect();
            Arc::new(under(sketch))
        };
        let ids: Vec<FeatureId> = (1..=11)
            .map(|k| FeatureId::new(&format!("s{k:07}")).unwrap())
            .collect();
        let reads: Vec<Arc<SketchRead>> = (0..11).map(|_| sketch()).collect();
        let camera = Camera {
            projection: Projection::Orthographic {
                view_height: 0.000_8,
            },
            ..at_120()
        };
        let mut lines = ShownLines::default();
        let mut drawn = lines.frame(ids.iter().zip(&reads), &camera, 800.0);
        assert_eq!(lines.built, 2 * 199_875);
        let mut frames = 1;
        while lines.pending() {
            drawn = lines.frame(ids.iter().zip(&reads), &camera, 800.0);
            assert!(lines.built <= MAX_FRAME_LINE_BUILD, "{}", lines.built);
            frames += 1;
            assert!(frames <= 6, "settled in {frames} frames");
        }
        assert_eq!(drawn.len(), 10);
        assert!(drawn.iter().all(|line| line.strips.points() == 199_875));
        assert_eq!(lines.capped, BTreeSet::from([ids[10].clone()]));
        assert!(lines.made[&ids[10]].strips.is_none());
        lines.frame(ids.iter().zip(&reads), &camera, 800.0);
        assert_eq!(lines.built, 0);
    }

    #[test]
    fn sketches_left_out_dont_starve_one_that_fits() {
        // 20 points to draw and 20 to build a frame: the rectangle (8), the circle (22), left
        // out, never built or spent on, and the side (2), which fits what's left of both.
        // Every frame of ten: [8, 2] drawn, nothing pending, and from the second nothing
        // built.
        let fid = |text: &str| FeatureId::new(text).unwrap();
        let (rect, circle, one) = (fid("s0000001"), fid("s0000002"), fid("s0000003"));
        let reads = [Arc::new(loose()), Arc::new(round()), Arc::new(side())];
        let shown = [(&rect, &reads[0]), (&circle, &reads[1]), (&one, &reads[2])];
        let budget = Budget {
            draw: 20,
            build: 20,
        };
        let mut lines = ShownLines::default();
        for frame in 0..10 {
            let drawn = lines.frame_within(shown, &at_120(), 800.0, budget);
            assert_eq!(points(&drawn), [8, 2], "frame {frame}");
            assert!(!lines.pending(), "frame {frame}");
            let built = if frame == 0 { 10 } else { 0 };
            assert_eq!(lines.built, built, "frame {frame}");
        }
    }

    #[test]
    fn a_set_too_big_to_draw_whole_settles_in_a_few_frames_and_then_builds_nothing() {
        // 100 to draw and 50 to build a frame: the rectangle (8), six circles (22 each), the
        // side (2). Building two circles' worth a frame, it settles by the third frame: the
        // rectangle, four circles and the side drawn (98); the last two circles out, never
        // built. Then frame after frame nothing is built and nothing pending.
        let ids: Vec<FeatureId> = (1..=8)
            .map(|k| FeatureId::new(&format!("s{k:07}")).unwrap())
            .collect();
        let reads: Vec<Arc<SketchRead>> = std::iter::once(Arc::new(loose()))
            .chain((0..6).map(|_| Arc::new(round())))
            .chain(std::iter::once(Arc::new(side())))
            .collect();
        let budget = Budget {
            draw: 100,
            build: 50,
        };
        let mut lines = ShownLines::default();
        let mut frames = 0;
        let mut drawn;
        loop {
            drawn = lines.frame_within(ids.iter().zip(&reads), &at_120(), 800.0, budget);
            frames += 1;
            if !lines.pending() {
                break;
            }
            assert!(frames < 3, "pending after {frames} frames");
        }
        assert_eq!(points(&drawn), [8, 22, 22, 22, 22, 2]);
        assert_eq!(
            lines.capped,
            BTreeSet::from([ids[5].clone(), ids[6].clone()])
        );
        for _ in 0..3 {
            lines.frame_within(ids.iter().zip(&reads), &at_120(), 800.0, budget);
            assert_eq!(lines.built, 0);
            assert!(!lines.pending());
        }
    }

    #[test]
    fn a_build_the_renderer_refuses_is_charged_its_count() {
        // Two sketches, each one side from the origin to x = 2·10¹² mm, past the renderer's
        // 10¹² reach: 2 points each, refused when built. Building 2 a frame: the first frame
        // builds one, charged its 2, and leaves the other for the next. Neither is drawn.
        let far = || {
            let mut sketch = rectangle(Vec::new());
            sketch.lines.truncate(1);
            sketch.points[1] = point("pa000002", 2e12, 0.0);
            Arc::new(under(sketch))
        };
        let ids: Vec<FeatureId> = (1..=2)
            .map(|k| FeatureId::new(&format!("s{k:07}")).unwrap())
            .collect();
        let reads = [far(), far()];
        let budget = Budget {
            draw: MAX_FRAME_LINE_POINTS,
            build: 2,
        };
        let mut lines = ShownLines::default();
        let drawn = lines.frame_within(ids.iter().zip(&reads), &at_120(), 800.0, budget);
        assert!(drawn.is_empty());
        assert_eq!(lines.built, 2);
        assert!(lines.pending());
        assert!(matches!(
            lines.made[&ids[0]].strips,
            Some((_, Err(NotDrawn::Refused(_))))
        ));
        let drawn = lines.frame_within(ids.iter().zip(&reads), &at_120(), 800.0, budget);
        assert!(drawn.is_empty());
        assert_eq!(lines.built, 2);
        assert!(!lines.pending());
    }

    #[test]
    fn a_construction_curves_count_is_never_below_what_is_made() {
        // A construction line of 40 mm, at 0.125 mm a point dashes of 0.625 mm and gaps of
        // 0.375 (a period of 1 mm): counted as its 2 vertices and 2 ends for each of 40
        // dashes, 82; made, 40 dashes of 2 points, 80. And a construction circle of radius
        // 1 m, an arc of radius 10 about the origin from (10, 0) to (0, 10), and the line, at
        // scales from a millionth of a millimetre a point (every curve at its most segments)
        // to 4: the count never below what's made.
        let mut line = rectangle(Vec::new());
        line.lines.truncate(1);
        line.lines[0].construction = true;
        let line = under(line);
        assert_eq!(count_points(&line, Scale(0.125)), Some(82));
        assert_eq!(
            strips_of(&line, Scale(0.125)).unwrap().unwrap().points(),
            80
        );
        let mut sketch = rectangle(Vec::new());
        sketch.lines.truncate(1);
        sketch.lines[0].construction = true;
        sketch.points.extend([
            point("pa000010", 0.0, 0.0),
            point("pa000011", 10.0, 0.0),
            point("pa000012", 0.0, 10.0),
        ]);
        sketch.circles.push(SketchCircle {
            id: id("ca000001"),
            center: id("pa000010"),
            radius: Coordinate::new(1000.0).unwrap(),
            construction: true,
        });
        sketch.arcs.push(silico_core::document::SketchArc {
            id: id("aa000001"),
            center: id("pa000010"),
            start: id("pa000011"),
            end: id("pa000012"),
            construction: true,
        });
        let read = under(sketch);
        for scale in [1e-6, 1e-3, 0.0625, 0.125, 1.0, 4.0] {
            let count = count_points(&read, Scale(scale)).unwrap();
            let made = strips_of(&read, Scale(scale)).unwrap().unwrap().points();
            assert!(count >= made, "at {scale}: {count} < {made}");
        }
    }

    #[test]
    fn sketches_of_construction_curves_left_out_make_no_polyline() {
        // Three sketches of 50 construction circles of 1 m, at a millionth of a millimetre a
        // point each counted far past the 20 points a frame may draw: left out, counted
        // without a polyline made or a point built.
        let sketch = || {
            let mut sketch = rectangle(Vec::new());
            sketch.lines.clear();
            sketch.points = vec![point("pa000006", 0.0, 0.0)];
            sketch.circles = (0..50)
                .map(|k| SketchCircle {
                    id: id(&format!("c{k:07}")),
                    center: id("pa000006"),
                    radius: Coordinate::new(1000.0).unwrap(),
                    construction: true,
                })
                .collect();
            Arc::new(under(sketch))
        };
        let ids: Vec<FeatureId> = (1..=3)
            .map(|k| FeatureId::new(&format!("s{k:07}")).unwrap())
            .collect();
        let reads = [sketch(), sketch(), sketch()];
        let camera = Camera {
            projection: Projection::Orthographic {
                view_height: 0.000_8,
            },
            ..at_120()
        };
        let budget = Budget {
            draw: 20,
            build: 20,
        };
        let mut lines = ShownLines::default();
        super::super::draw::POLYLINES_MADE.with(|made| made.set(0));
        let drawn = lines.frame_within(ids.iter().zip(&reads), &camera, 800.0, budget);
        assert!(drawn.is_empty());
        assert_eq!(
            super::super::draw::POLYLINES_MADE.with(std::cell::Cell::get),
            0
        );
        assert_eq!(lines.built, 0);
        assert_eq!(lines.capped.len(), 3);
    }

    #[test]
    fn a_sketch_of_a_million_lines_is_refused_at_once_and_draws_nothing() {
        // 500,000 lines, a million points, past the cap: none drawn.
        let mut sketch = rectangle(Vec::new());
        sketch.points = (0..500_001u32)
            .map(|k| point(&format!("p{k:07}"), f64::from(k), 0.0))
            .collect();
        sketch.lines = (0..500_000u32)
            .map(|k| {
                line(
                    &format!("n{k:07}"),
                    &format!("p{k:07}"),
                    &format!("p{:07}", k + 1),
                )
            })
            .collect();
        let read = Arc::new(under(sketch));
        let mut shown = ShownLines::default();
        let camera = Camera {
            projection: Projection::Orthographic { view_height: 120.0 },
            eye: [0.0, -100.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        };
        let id = FeatureId::new("s0000002").unwrap();
        assert!(shown.frame([(&id, &read)], &camera, 800.0).is_empty());
        assert_eq!(
            strips_of(&read, Scale(0.125)).map(|made| made.err()),
            Some(Some(NotDrawn::TooMany))
        );
    }

    /// Loose's rectangle on XY with a circle of radius 3 about (20, 1): the circle's square,
    /// 17 to 23 by −2 to 4, reaches 2 below the rectangle, which no point does.
    fn off_centre() -> SketchRead {
        let mut sketch = rectangle(Vec::new());
        sketch.points.push(point("pa000006", 20.0, 1.0));
        sketch.circles.push(SketchCircle {
            id: id("ca000001"),
            center: id("pa000006"),
            radius: Coordinate::new(3.0).unwrap(),
            construction: false,
        });
        under(sketch)
    }

    /// `n` lines end to end along x, from (0, 0) to (n, 0): 2n points.
    fn chain(n: u32) -> SketchRead {
        let mut sketch = rectangle(Vec::new());
        sketch.points = (0..=n)
            .map(|k| point(&format!("p{k:07}"), f64::from(k), 0.0))
            .collect();
        sketch.lines = (0..n)
            .map(|k| {
                line(
                    &format!("n{k:07}"),
                    &format!("p{k:07}"),
                    &format!("p{:07}", k + 1),
                )
            })
            .collect();
        under(sketch)
    }

    /// One line from `from` to `to`, construction or not, alone on XY.
    fn lone_line(from: [f64; 2], to: [f64; 2], construction: bool) -> SketchRead {
        let mut sketch = rectangle(Vec::new());
        sketch.points = vec![
            point("pa000001", from[0], from[1]),
            point("pa000002", to[0], to[1]),
        ];
        sketch.lines.truncate(1);
        sketch.lines[0].construction = construction;
        under(sketch)
    }

    #[test]
    fn a_reads_box_is_its_points_and_each_round_curves_square_placed_by_its_plane() {
        // The small circle alone, radius 2.75 about (40, 12) on XY: its square, 40 ± 2.75 by
        // 12 ± 2.75, each side the radius from its centre (the one point), at z = 0.
        assert_eq!(
            box_of(&round()),
            Some(([37.25, 9.25, 0.0], [42.75, 14.75, 0.0]))
        );
        // The rectangle (0 to 40 by 0 to 20) with the circle about (20, 1): x 0 to 40, y from
        // the circle's −2 to the rectangle's 20. On XZ, (x, y) at (x, 0, y).
        let xz = SketchRead {
            plane: Ok(OriginPlane::Xz.into()),
            ..off_centre()
        };
        assert_eq!(box_of(&xz), Some(([0.0, 0.0, -2.0], [40.0, 0.0, 20.0])));
        // On a face frame at (100, 0, 0), its x along −y and its y along +z: the square's x,
        // 37.25 to 42.75, runs from y = −42.75 to −37.25; its y, 9.25 to 14.75, along z.
        let face = SketchRead {
            plane: Ok(silico_core::kernel::SketchPlane::Face(
                silico_protocol::FaceFrame {
                    origin: [100.0, 0.0, 0.0],
                    x: [0.0, -1.0, 0.0],
                    y: [0.0, 0.0, 1.0],
                },
            )),
            ..round()
        };
        assert_eq!(
            box_of(&face),
            Some(([100.0, -42.75, 9.25], [100.0, -37.25, 14.75]))
        );
        // No point, so no curve: no box.
        let mut empty = rectangle(Vec::new());
        empty.points.clear();
        empty.lines.clear();
        assert_eq!(box_of(&under(empty)), None);
    }

    #[test]
    fn a_scale_is_none_for_no_positive_height_and_on_a_slant_taken_where_x_and_y_are_lowest() {
        let ortho = |view_height: f64| Camera {
            projection: Projection::Orthographic { view_height },
            eye: [0.0, -100.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        };
        let anywhere = ([0.0; 3], [1.0; 3]);
        // A view of no height: 0 / 800 = 0 mm a point, none. A view of negative height,
        // −120 / 800 = −0.15, none; nor over a pane of negative height, though −120 / −800
        // is 0.15. A view infinitely high: none.
        assert_eq!(scale_at(&ortho(0.0), 800.0, anywhere), None);
        assert_eq!(scale_at(&ortho(-120.0), 800.0, anywhere), None);
        assert_eq!(scale_at(&ortho(-120.0), -800.0, anywhere), None);
        assert_eq!(scale_at(&ortho(f64::INFINITY), 800.0, anywhere), None);
        // 60° vertical, from the origin toward (30, 40, 0): the view (0.6, 0.8, 0), a corner's
        // depth 0.6x + 0.8y. The box (10, 5, −1) to (20, 30, 1): its corners at depths 10 (x
        // and y lowest), 16 (x highest, y lowest), 30 and 36; the nearest, 10. There the view
        // is 2 · 10 · tan 30° = 20/√3 high, over 200 points 0.1/√3 mm a point.
        let slant = Camera {
            projection: Projection::Perspective {
                field_of_view: 60.0,
            },
            eye: [0.0; 3],
            target: [30.0, 40.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        let expected = 0.1 / 3.0_f64.sqrt();
        let scale = scale_at(&slant, 200.0, ([10.0, 5.0, -1.0], [20.0, 30.0, 1.0])).unwrap();
        assert!((scale.0 - expected).abs() < 1e-12, "{scale:?}");
        // The box given highest corner first: the same eight corners, the same scale.
        let swapped = scale_at(&slant, 200.0, ([20.0, 30.0, 1.0], [10.0, 5.0, -1.0])).unwrap();
        assert!((swapped.0 - expected).abs() < 1e-12, "{swapped:?}");
    }

    #[test]
    fn a_perspectives_bucket_is_taken_at_the_side_only_a_circles_square_reaches() {
        // The rectangle with the circle of radius 3 about (20, 1), one sketch: 8 points and
        // the circle's. Orthographic, 240 mm over 800 points: 0.3, bucket 0.25; the circle 12
        // points in radius, each chord's angle 2·acos(1 − 0.25/12) = 0.40896, 2π over it 15.36,
        // so 16 segments: 8 + 17 = 25.
        let sid = FeatureId::new("s0000001").unwrap();
        let read = Arc::new(off_centre());
        let mut lines = ShownLines::default();
        let ortho = Camera {
            projection: Projection::Orthographic { view_height: 240.0 },
            ..at_120()
        };
        assert_eq!(points(&lines.frame([(&sid, &read)], &ortho, 800.0)), [25]);
        // 90° vertical from (20, −60, 80) toward (20, 0, 0): the view (0, 0.6, −0.8), a point
        // (x, y, 0) at depth 0.6(y + 60) + 64. Its nearest, y = −2 (the circle's square's, the
        // rectangle's lowest being 0), at 98.8: 2 · 98.8 · tan 45° / 795 = 0.24855, bucket
        // 0.125 (at y = 0, 100, it would be 0.25157, bucket 0.25). The circle 24 points in
        // radius, 2·acos(1 − 0.25/24) = 0.28893, 2π over it 21.75: 22 segments, 8 + 23 = 31.
        let slant = Camera {
            projection: Projection::Perspective {
                field_of_view: 90.0,
            },
            eye: [20.0, -60.0, 80.0],
            target: [20.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        assert_eq!(points(&lines.frame([(&sid, &read)], &slant, 795.0)), [31]);
        assert_eq!(lines.built, 31);
    }

    #[test]
    fn each_curves_count_is_its_polylines_and_a_construction_curves_bounds_its_dashes() {
        // At 0.125 mm a point: dashes of 0.625 mm and gaps of 0.375, a period of 1 mm.
        let at = Scale(0.125);
        // A line: its 2 ends.
        assert_eq!(count_points(&side(), at), Some(2));
        // The circle of radius 2.75: 22 points in radius, 2·acos(1 − 0.25/22) = 0.30180, 2π
        // over it 20.82, so 21 segments and 22 points.
        assert_eq!(count_points(&round(), at), Some(22));
        // A quarter arc of radius 10 about the origin, (10, 0) to (0, 10): 80 points in
        // radius, 2·acos(1 − 0.25/80) = 0.15816, π/2 over it 9.93, so 10 segments and 11
        // points. As construction, 11 and 2 ends for each of ⌈10 · π/2⌉ = ⌈15.71⌉ = 16
        // dashes: 43.
        let arc = |construction: bool| {
            let mut sketch = rectangle(Vec::new());
            sketch.lines.clear();
            sketch.points = vec![
                point("pa000010", 0.0, 0.0),
                point("pa000011", 10.0, 0.0),
                point("pa000012", 0.0, 10.0),
            ];
            sketch.arcs.push(silico_core::document::SketchArc {
                id: id("aa000001"),
                center: id("pa000010"),
                start: id("pa000011"),
                end: id("pa000012"),
                construction,
            });
            under(sketch)
        };
        assert_eq!(count_points(&arc(false), at), Some(11));
        assert_eq!(count_points(&arc(true), at), Some(43));
        // The circle as construction: 22 and 2 for each of ⌈2.75 · 2π⌉ = ⌈17.28⌉ = 18: 58.
        let mut circle = round();
        circle.members.circles[0].construction = true;
        assert_eq!(count_points(&circle, at), Some(58));
        // A construction line from (10, 20) to (40, 60), 30 by 40, so 50 long: 2 and 2 for
        // each of 50 dashes, 102 (of x 10 + 40 or y 20 + 60 it would be 132 or 174). Made, 50
        // dashes of 2 points.
        let slanted = lone_line([10.0, 20.0], [40.0, 60.0], true);
        assert_eq!(count_points(&slanted, at), Some(102));
        assert_eq!(strips_of(&slanted, at).unwrap().unwrap().points(), 100);
        // A construction line 2,000 long: 2,000 dashes, past MAX_CURVE_DASHES, so 1,024 and 1
        // at most: 2 + 2 · 1,025 = 2,052. Made, 1,024 dashes stretched to fit, 2,048 points.
        let long = lone_line([0.0, 0.0], [2000.0, 0.0], true);
        assert_eq!(count_points(&long, at), Some(2052));
        assert_eq!(strips_of(&long, at).unwrap().unwrap().points(), 2048);
    }

    #[test]
    fn a_count_stops_at_the_first_curve_past_the_cap() {
        // 100,000 lines: 200,000 points, the cap exactly, every one counted. 100,001: the
        // last takes it to 200,002, past the cap. 150,000: the same 200,002, the count
        // stopped there, not the 300,000 of all of them.
        let at = Scale(0.125);
        assert_eq!(count_points(&chain(100_000), at), Some(200_000));
        assert_eq!(count_points(&chain(100_001), at), Some(200_002));
        assert_eq!(count_points(&chain(150_000), at), Some(200_002));
    }

    #[test]
    fn a_sketch_of_exactly_its_cap_is_drawn_and_one_more_line_is_neither_built_nor_drawn() {
        // 100,000 lines, 200,000 points (MAX_LINE_POINTS): made whole, built and drawn in one
        // frame (500,000 a frame may build, 2,000,000 draw). 100,001, 200,002 points: counted
        // past the cap, so no polyline made, nothing built or drawn.
        let at = Scale(0.125);
        let exact = Arc::new(chain(100_000));
        let made = strips_of(&exact, at).unwrap().unwrap();
        assert_eq!((made.len(), made.points()), (100_000, 200_000));
        let sid = FeatureId::new("s0000001").unwrap();
        let mut lines = ShownLines::default();
        assert_eq!(
            points(&lines.frame([(&sid, &exact)], &at_120(), 800.0)),
            [200_000]
        );
        assert_eq!(lines.built, 200_000);
        let past = Arc::new(chain(100_001));
        assert_eq!(
            strips_of(&past, at).map(|made| made.err()),
            Some(Some(NotDrawn::TooMany))
        );
        let mut lines = ShownLines::default();
        super::super::draw::POLYLINES_MADE.with(|made| made.set(0));
        assert!(lines.frame([(&sid, &past)], &at_120(), 800.0).is_empty());
        assert_eq!(lines.built, 0);
        assert_eq!(
            super::super::draw::POLYLINES_MADE.with(std::cell::Cell::get),
            0
        );
    }

    #[test]
    fn a_new_bucket_counts_and_builds_again_and_the_same_bucket_does_neither() {
        // The circle of radius 2.75. At 120 mm over 800 points (0.15, bucket 0.125): 22
        // points, built. At 60 (0.075, bucket 0.0625): 44 points in radius,
        // 2·acos(1 − 0.25/44) = 0.21330, 2π over it 29.46, 30 segments: 31 points, counted
        // and built again. At 55 (0.06875, the same bucket): nothing built, the same strips.
        // Back at 120: 22, counted and built again.
        let view = |view_height: f64| Camera {
            projection: Projection::Orthographic { view_height },
            ..at_120()
        };
        let sid = FeatureId::new("s0000001").unwrap();
        let read = Arc::new(round());
        let mut lines = ShownLines::default();
        assert_eq!(
            points(&lines.frame([(&sid, &read)], &view(120.0), 800.0)),
            [22]
        );
        assert_eq!(lines.built, 22);
        let finer = lines.frame([(&sid, &read)], &view(60.0), 800.0);
        assert_eq!((points(&finer), lines.built), (vec![31], 31));
        let same = lines.frame([(&sid, &read)], &view(55.0), 800.0);
        assert_eq!(lines.built, 0);
        assert!(Arc::ptr_eq(&finer[0].strips, &same[0].strips));
        let back = lines.frame([(&sid, &read)], &view(120.0), 800.0);
        assert_eq!((points(&back), lines.built), (vec![22], 22));
    }

    #[test]
    fn a_sketch_exactly_filling_whats_left_to_draw_is_drawn() {
        // 10 points to draw: the rectangle (8), then the side (2), which fills the 2 left.
        let (rect, one) = (
            FeatureId::new("s0000001").unwrap(),
            FeatureId::new("s0000002").unwrap(),
        );
        let reads = [Arc::new(loose()), Arc::new(side())];
        let budget = Budget {
            draw: 10,
            build: MAX_FRAME_LINE_BUILD,
        };
        let mut lines = ShownLines::default();
        let drawn = lines.frame_within(
            [(&rect, &reads[0]), (&one, &reads[1])],
            &at_120(),
            800.0,
            budget,
        );
        assert_eq!(points(&drawn), [8, 2]);
        assert!(lines.capped.is_empty());
    }

    #[test]
    fn a_sketch_past_a_whole_frames_build_is_built_while_nothing_else_has_been() {
        // 5 points a frame to build: the rectangle (8) is past it, but the frame has built
        // nothing yet, so it's built (8); the side (2), past the 0 then left, waits for the
        // next frame, which builds it (2).
        let (rect, one) = (
            FeatureId::new("s0000001").unwrap(),
            FeatureId::new("s0000002").unwrap(),
        );
        let reads = [Arc::new(loose()), Arc::new(side())];
        let shown = [(&rect, &reads[0]), (&one, &reads[1])];
        let budget = Budget {
            draw: MAX_FRAME_LINE_POINTS,
            build: 5,
        };
        let mut lines = ShownLines::default();
        let first = lines.frame_within(shown, &at_120(), 800.0, budget);
        assert_eq!((points(&first), lines.built), (vec![8], 8));
        assert!(lines.pending());
        let second = lines.frame_within(shown, &at_120(), 800.0, budget);
        assert_eq!((points(&second), lines.built), (vec![8, 2], 2));
        assert!(!lines.pending());
    }

    #[test]
    fn a_sketch_whose_strips_are_empty_draws_no_lines() {
        // The rectangle's points without its lines: no curve, so 0 points counted and an
        // empty set of strips. A construction line of no length, from (5, 5) to (5, 5):
        // counted as its 2 ends and no dash, built (2), no dash made. Neither is drawn, and
        // neither is left out.
        let mut points_only = rectangle(Vec::new());
        points_only.lines.clear();
        let (dots, nil) = (
            FeatureId::new("s0000001").unwrap(),
            FeatureId::new("s0000002").unwrap(),
        );
        let reads = [
            Arc::new(under(points_only)),
            Arc::new(lone_line([5.0, 5.0], [5.0, 5.0], true)),
        ];
        let mut lines = ShownLines::default();
        let drawn = lines.frame([(&dots, &reads[0]), (&nil, &reads[1])], &at_120(), 800.0);
        assert!(drawn.is_empty(), "{:?}", points(&drawn));
        assert_eq!(lines.built, 2);
        assert!(lines.capped.is_empty());
    }

    #[test]
    fn a_pattern_past_the_most_dashes_is_stretched_by_its_length_over_its_period() {
        // 4,096 long, dashes of 1 and gaps of 1: 2,048 periods of 2, past 1,024, so both are
        // stretched by 4,096 / 2 / 1,024 = 2: dashes of 2 every 4, 1,024 of them, the kth from
        // 4k to 4k + 2.
        let expected: Vec<Vec<[f64; 2]>> = (0..1024)
            .map(|k| {
                let start = 4.0 * f64::from(k);
                vec![[start, 0.0], [start + 2.0, 0.0]]
            })
            .collect();
        assert_eq!(dashes(&[[0.0, 0.0], [4096.0, 0.0]], 1.0, 1.0), expected);
    }

    #[test]
    fn dashes_follow_chords_off_the_axes() {
        // Two chords, (0, 0) to (3, 4) and on to (6, 0), each 5 long. Dashes of 2.5 and gaps
        // of 2.5: one along each chord's first half, [(0, 0), (1.5, 2)] and [(3, 4), (4.5,
        // 2)]. Dashes of 7.5 and gaps of 2.5: one, over the corner to the second chord's
        // middle, (4.5, 2).
        let chords = [[0.0, 0.0], [3.0, 4.0], [6.0, 0.0]];
        let near = |made: Vec<Vec<[f64; 2]>>, expected: &[&[[f64; 2]]]| {
            assert_eq!(made.len(), expected.len(), "{made:?}");
            for (dash, want) in made.iter().zip(expected) {
                assert_eq!(dash.len(), want.len(), "{made:?}");
                for (p, q) in dash.iter().zip(want.iter()) {
                    let off = libm::hypot(p[0] - q[0], p[1] - q[1]);
                    assert!(off < 1e-12, "{made:?}");
                }
            }
        };
        near(
            dashes(&chords, 2.5, 2.5),
            &[&[[0.0, 0.0], [1.5, 2.0]], &[[3.0, 4.0], [4.5, 2.0]]],
        );
        near(
            dashes(&chords, 7.5, 2.5),
            &[&[[0.0, 0.0], [3.0, 4.0], [4.5, 2.0]]],
        );
        // An L a unit above the axis, (0, 1) to (4, 1) to (4, 5), dashes and gaps of 1: two
        // along each side, every end a quarter of the way along one.
        assert_eq!(
            dashes(&[[0.0, 1.0], [4.0, 1.0], [4.0, 5.0]], 1.0, 1.0),
            vec![
                vec![[0.0, 1.0], [1.0, 1.0]],
                vec![[2.0, 1.0], [3.0, 1.0]],
                vec![[4.0, 1.0], [4.0, 2.0]],
                vec![[4.0, 3.0], [4.0, 4.0]],
            ]
        );
    }

    #[test]
    fn a_stretch_rounded_short_of_the_length_leaves_one_sliver_dash_more() {
        // Dashes of 0.2 and gaps of 0.1 along 385: a period of fl(0.2 + 0.1) =
        // 0.30000000000000004, 1,283.3 of them, past 1,024, so both are stretched by
        // fl(385 / 0.30000000000000004) / 1,024. The quotient, 1,283.333333333333, is
        // rounded down by 1.13e-13 (just under half its ulp, 2⁻⁴²); the stretched period,
        // 0.30000000000000004 times it over 1,024, is exactly 385/1,024 less 3.3e-17, which
        // rounds down a whole ulp (2⁻⁵⁴) to 0.37597656249999994. 1,024 periods reach
        // 385 − 2⁻⁴⁴ (384.99999999999994, an ulp of 385 short of it): the 1,025th dash is the
        // sliver from there to 385, the one dash past MAX_CURVE_DASHES a curve may have.
        // (Worked in exact rationals.)
        let made = dashes(&[[0.0, 0.0], [385.0, 0.0]], 0.2, 0.1);
        assert_eq!(made.len(), MAX_CURVE_DASHES + 1);
        assert_eq!(
            made[MAX_CURVE_DASHES],
            vec![[385.0 - libm::ldexp(1.0, -44), 0.0], [385.0, 0.0]]
        );
    }

    #[test]
    fn a_pattern_whose_period_overflows_makes_no_point_that_isnt_finite() {
        // Dashes and gaps of 10³⁰⁸, whose sum overflows; or dashes of 10⁻³⁰⁰ along 10¹⁰,
        // whose count of periods overflows, so its stretch: either way once an infinite
        // period, whose first dash's start, 0 · ∞, was no number. No dash has a point that
        // isn't finite.
        for made in [
            dashes(&[[0.0, 0.0], [1.0, 0.0]], 1e308, 1e308),
            dashes(&[[0.0, 0.0], [1e10, 0.0]], 1e-300, 0.0),
        ] {
            assert!(
                made.iter().flatten().flatten().all(|c| c.is_finite()),
                "{made:?}"
            );
        }
    }

    #[test]
    fn a_period_past_f64s_range_or_counted_past_it_still_makes_dashes() {
        // Dashes of 10³⁰⁸ and gaps of 10³⁰⁸ along a unit line: their sum overflows, but a
        // period longer than the line is one dash covering it.
        assert_eq!(
            dashes(&[[0.0, 0.0], [1.0, 0.0]], 1e308, 1e308),
            vec![vec![[0.0, 0.0], [1.0, 0.0]]]
        );
        // Dashes of 2¹⁰²² and gaps of 3 · 2¹⁰²² (a sum of 2¹⁰²⁴, past f64's range) along a
        // line 2¹⁰²³ long: one dash, the first half of it, and then the gap past its end.
        let (half, whole) = (libm::ldexp(1.0, 1022), libm::ldexp(1.0, 1023));
        assert_eq!(
            dashes(&[[0.0, 0.0], [whole, 0.0]], half, 3.0 * half),
            vec![vec![[0.0, 0.0], [half, 0.0]]]
        );
        // Dashes of 2⁻¹⁰⁷⁴ (f64's least) and no gap along 1,024: 2¹⁰⁸⁴ periods, a count past
        // f64's range, so 1,024 periods of 1, each all dash: from k to k + 1.
        let tiny = libm::ldexp(1.0, -1074);
        let solid = dashes(&[[0.0, 0.0], [1024.0, 0.0]], tiny, 0.0);
        assert_eq!(solid.len(), MAX_CURVE_DASHES);
        for (k, dash) in solid.iter().enumerate() {
            let k = k as f64;
            assert_eq!(dash, &vec![[k, 0.0], [k + 1.0, 0.0]]);
        }
        // Dashes of 2⁻¹⁰⁷⁴ and gaps three times as long along 1,024: periods of 2⁻¹⁰⁷², 2¹⁰⁸²
        // of them, so 1,024 periods of 1, each a quarter dash: from k to k + 0.25.
        let quarter = dashes(&[[0.0, 0.0], [1024.0, 0.0]], tiny, 3.0 * tiny);
        assert_eq!(quarter.len(), MAX_CURVE_DASHES);
        for (k, dash) in quarter.iter().enumerate() {
            let k = k as f64;
            assert_eq!(dash, &vec![[k, 0.0], [k + 0.25, 0.0]]);
        }
    }
}
