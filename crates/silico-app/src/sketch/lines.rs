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
//!   sketch past [`MAX_LINE_POINTS`], which a warning names.
//! - SEAM: Fit and Home fit the bodies alone (the core's named views, a core change); the
//!   camera's clamp takes the lines' boxes.

use std::collections::BTreeMap;
use std::sync::Arc;

use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, FeatureId};
use silico_core::sketch_states::SketchState;
use silico_protocol::vec3::{dot, norm, sub};
use silico_protocol::{Camera, Projection};
use silico_render::render::Strips;

use super::draw::{Curve, Geometry, Scale, fill_scale, polyline};
use super::style::SKETCH_CHORD_TOLERANCE;
use crate::theme;

/// The most points one sketch's lines may have over its strips: past it the sketch isn't
/// drawn, and a warning says which, as a region's fill past `MAX_FILL_VERTICES` is left out.
pub const MAX_LINE_POINTS: usize = 200_000;

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
    if !(height > 0.0) {
        return None;
    }
    let scale = match camera.projection {
        Projection::Orthographic { view_height } => view_height / height,
        Projection::Perspective { field_of_view } => {
            let towards = sub(camera.target, camera.eye);
            let focus = norm(towards);
            if !(focus > 0.0) {
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
            if !(farthest > 0.0) {
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
    let members = &read.members;
    let construction: BTreeMap<&ElementId, bool> = members
        .lines
        .iter()
        .map(|l| (&l.id, l.construction))
        .chain(members.circles.iter().map(|c| (&c.id, c.construction)))
        .chain(members.arcs.iter().map(|a| (&a.id, a.construction)))
        .collect();
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

/// A shown sketch's lines as the frame gives them the renderer: its strips, shared with
/// the cache that made them, and its colour; its width is the theme's.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameLines {
    pub strips: Arc<Strips>,
    pub colour: [u8; 4],
}

/// What was made of a sketch's read: the read (so a replaced one is seen by its `Arc`), its
/// box ([`box_of`], none for one its plane doesn't place), and, at the bucket they were made
/// at, the strips or why there are none.
struct Made {
    read: Arc<SketchRead>,
    bounds: Option<([f64; 3], [f64; 3])>,
    strips: Option<(i32, Result<Arc<Strips>, NotDrawn>)>,
}

/// Each shown sketch's lines, kept from frame to frame: a sketch's made again only when
/// its read is replaced or the view's scale at it leaves its bucket.
#[derive(Default)]
pub struct ShownLines {
    made: BTreeMap<FeatureId, Made>,
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
    /// camera's clamp.
    pub fn frame<'a>(
        &mut self,
        shown: impl IntoIterator<Item = (&'a FeatureId, &'a Arc<SketchRead>)>,
        camera: &Camera,
        height: f32,
    ) -> Vec<FrameLines> {
        let mut kept = BTreeMap::new();
        let mut lines = Vec::new();
        for (id, read) in shown {
            // The box worked out once a read; the strips once a read and a bucket.
            let mut made = match self.made.remove(id) {
                Some(made) if Arc::ptr_eq(&made.read, read) => made,
                _ => Made {
                    read: Arc::clone(read),
                    bounds: box_of(read),
                    strips: None,
                },
            };
            let scale = made
                .bounds
                .and_then(|bounds| scale_at(camera, height, bounds));
            if let Some(scale) = scale {
                let (bucket, at) = fill_scale(scale);
                if made
                    .strips
                    .as_ref()
                    .is_none_or(|(made_at, _)| *made_at != bucket)
                {
                    let strips = strips_of(read, at).map(|strips| strips.map(Arc::new));
                    if let Some(Err(why)) = &strips {
                        tracing::warn!(sketch = %id, ?why, "a shown sketch's lines aren't drawn");
                    }
                    made.strips = strips.map(|strips| (bucket, strips));
                }
            }
            if let Some((_, Ok(strips))) = &made.strips
                && !strips.is_empty()
            {
                lines.push(FrameLines {
                    strips: Arc::clone(strips),
                    colour: colour(read.states.state),
                });
            }
            kept.insert(id.clone(), made);
        }
        self.made = kept;
        lines
    }
}

/// The most dashes one curve is cut into: past it its dashes and gaps are lengthened alike
/// to fit, so a long construction curve seen closely costs no more than its outline.
pub const MAX_CURVE_DASHES: usize = super::draw::MAX_CURVE_SEGMENTS;

/// `polyline` cut into dashes `on` long with gaps `off` long between them, measured along
/// it from its start, each dash a polyline of its own (its ends on the polyline, and every
/// vertex between them): how a construction curve is drawn. Where that would make more than
/// [`MAX_CURVE_DASHES`], both are lengthened by one factor to make that many. A polyline of
/// no length, or a dash or gap that isn't a finite length above 0 (the dash) or at least 0
/// (the gap), makes none.
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
    let stretch = (total / (on + off) / most).max(1.0);
    let (on, period) = (on * stretch, (on + off) * stretch);
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
        if !(start < total) {
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

    #[test]
    fn a_sketch_of_a_million_lines_is_refused_at_once_and_draws_nothing() {
        // 500,000 lines, a million points, past the cap: none drawn, and no time spent
        // placing what isn't drawn.
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
        let started = std::time::Instant::now();
        let mut shown = ShownLines::default();
        let camera = Camera {
            projection: Projection::Orthographic { view_height: 120.0 },
            eye: [0.0, -100.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        };
        let id = FeatureId::new("s0000002").unwrap();
        assert!(shown.frame([(&id, &read)], &camera, 800.0).is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert_eq!(
            strips_of(&read, Scale(0.125)).map(|made| made.err()),
            Some(Some(NotDrawn::TooMany))
        );
    }
}
