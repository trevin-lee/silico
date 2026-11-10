//! The viewport's camera controls (docs/design/m5.0-app.md, section 8, *Navigation*;
//! docs/ux/fusion.md, *Navigation defaults* and *ViewCube and Navigation Bar*): orbit about
//! the target, pan, zoom toward the cursor, fit, the named views, and the projection. The
//! camera is the app's own (decision 29), held in `f64` as the core's [`Camera`], and never
//! reaches a document (section 11). Each control gives the camera it makes, or none when
//! that camera makes no frame ([`Camera::frame`]: non-finite, or past the least or the most
//! a camera may be; its angle's range at a size is the frame's own check, and the app's
//! perspective angle, [`PERSPECTIVE_FIELD_OF_VIEW`], is inside it at any pane but a sliver
//! (the narrowest angle a pane allows is 2·atan((W² + H²)/(H·2²⁰)): 30.3° at 3840 × 52, so
//! a pane under about 52 rows at 4K width refuses it, its frame not drawn), so the view
//! stays as it was; a fit or a named view, which has the pane's size, takes the whole check
//! at it. [`drawable`] says whether a renderer draws a camera's view of the bodies (an
//! orthographic view no smaller than their reach allows at the pane's height in pixels),
//! which the viewport asks of every camera a control makes.

use silico_core::views::{self, NamedView};
use silico_protocol::vec3::{cross, dot, sub};
use silico_protocol::{Camera, Projection, least_view_height};

/// How far a drag of one point orbits, in degrees: half a turn across 360 points.
pub const ORBIT_DEGREES_PER_POINT: f64 = 0.5;

/// How far the wheel scrolls, in points, to halve (or double) the view's size.
pub const ZOOM_POINTS_PER_OCTAVE: f64 = 200.0;

/// A perspective view's vertical angle, in degrees, when the view is switched to one.
pub const PERSPECTIVE_FIELD_OF_VIEW: f64 = 30.0;

/// The view Home gives, and the first one a document opens at: Fusion's home is a corner's
/// isometric view, its front and right toward the viewer, Z up.
pub const HOME: NamedView = NamedView::TopFrontRight;

/// What a view with no box to fit is fitted to: a 100 mm cube about the origin.
pub const NO_BOX: [[f64; 3]; 2] = [[-50.0; 3], [50.0; 3]];

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scaled(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// `v` turned by `angle` radians about the unit `axis` (Rodrigues' formula).
fn rotated(v: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    let (sin, cos) = (libm::sin(angle), libm::cos(angle));
    let along = dot(axis, v) * (1.0 - cos);
    add(
        add(scaled(v, cos), scaled(cross(axis, v), sin)),
        scaled(axis, along),
    )
}

/// `camera` if it makes a frame ([`Camera::frame`]).
fn checked(camera: Camera) -> Option<Camera> {
    camera.frame().ok().map(|_| camera)
}

/// `camera` if it passes its check at an image `width` by `height`.
fn checked_at(camera: Camera, width: u32, height: u32) -> Option<Camera> {
    camera.check(width, height).ok().map(|_| camera)
}

/// How many mm a point spans at the target's depth, in a view `height` points high: what a
/// pan moves the view by for each point dragged.
pub fn mm_per_point(camera: &Camera, height: f64) -> Option<f64> {
    let frame = camera.frame().ok()?;
    let view_height = match camera.projection {
        Projection::Orthographic { view_height } => view_height,
        Projection::Perspective { field_of_view } => {
            2.0 * frame.distance * libm::tan(field_of_view.to_radians() / 2.0)
        }
    };
    (height > 0.0 && height.is_finite()).then(|| view_height / height)
}

/// Orbits about the target by a drag of `dx`, `dy` points (y down, as the screen's), as
/// Fusion's constrained orbit turns a Z-up model: across, about the world's Z through the
/// target; up and down, about the view's right. The model turns with the pointer: a drag to
/// the right brings its left side into view, and a drag up its bottom.
pub fn orbit(camera: &Camera, dx: f64, dy: f64) -> Option<Camera> {
    let frame = camera.frame().ok()?;
    let per_point = ORBIT_DEGREES_PER_POINT.to_radians();
    let turn = |v: [f64; 3]| {
        let tilted = rotated(v, frame.right, -dy * per_point);
        rotated(tilted, [0.0, 0.0, 1.0], -dx * per_point)
    };
    let offset = turn(sub(camera.eye, camera.target));
    checked(Camera {
        eye: add(camera.target, offset),
        up: turn(frame.up),
        ..*camera
    })
}

/// Pans by a drag of `dx`, `dy` points (y down) in a view `height` points high: the model
/// follows the pointer at the target's depth.
pub fn pan(camera: &Camera, dx: f64, dy: f64, height: f64) -> Option<Camera> {
    let frame = camera.frame().ok()?;
    let mm = mm_per_point(camera, height)?;
    let shift = add(scaled(frame.right, -dx * mm), scaled(frame.up, dy * mm));
    checked(Camera {
        eye: add(camera.eye, shift),
        target: add(camera.target, shift),
        ..*camera
    })
}

/// Zooms by `scroll` points of the wheel (positive toward the model: in) toward the point
/// under the cursor, `cursor` points from the view's centre (y down), in a view `height`
/// points high: that point, at the target's depth, stays under the cursor (#196's M5.6;
/// docs/design/m5.0-app.md, section 8). The view's size scales by 2^(−scroll / 200).
pub fn zoom(camera: &Camera, scroll: f64, cursor: [f64; 2], height: f64) -> Option<Camera> {
    let frame = camera.frame().ok()?;
    let mm = mm_per_point(camera, height)?;
    let factor = libm::exp2(-scroll / ZOOM_POINTS_PER_OCTAVE);
    let under = add(
        camera.target,
        add(
            scaled(frame.right, cursor[0] * mm),
            scaled(frame.up, -cursor[1] * mm),
        ),
    );
    let about = |p: [f64; 3]| add(under, scaled(sub(p, under), factor));
    let target = about(camera.target);
    let (eye, projection) = match camera.projection {
        Projection::Orthographic { view_height } => (
            add(camera.eye, sub(target, camera.target)),
            Projection::Orthographic {
                view_height: view_height * factor,
            },
        ),
        Projection::Perspective { .. } => (about(camera.eye), camera.projection),
    };
    checked(Camera {
        projection,
        eye,
        target,
        up: camera.up,
    })
}

/// Fits the view to `bounds` in an image `width` by `height`, looking the way it looks: the
/// core's own fit along the view's direction and up (`views::fit_along`, which the named
/// views fit by), kept perspective if it is, its eye far enough back that the view at the
/// box's centre is that high, and half a diagonal more.
pub fn fit(camera: &Camera, bounds: [[f64; 3]; 2], width: u32, height: u32) -> Option<Camera> {
    fit_in(camera, bounds, width, height, Inset::default())
}

/// [`fit`], fitted into the part of the pane `inset` leaves: looking the way it looks, the
/// box at that part's centre, kept perspective if it is.
pub fn fit_in(
    camera: &Camera,
    bounds: [[f64; 3]; 2],
    width: u32,
    height: u32,
    inset: Inset,
) -> Option<Camera> {
    let frame = camera.frame().ok()?;
    let fitted = inset_fit(width, height, inset, |across, down| {
        views::fit_along(frame.back, frame.up, bounds, across, down).0
    })?;
    let Projection::Perspective { field_of_view } = camera.projection else {
        return checked_at(fitted, width, height);
    };
    let Projection::Orthographic { view_height } = fitted.projection else {
        return None;
    };
    let half_diagonal = {
        let d = sub(bounds[1], bounds[0]);
        dot(d, d).sqrt() / 2.0
    };
    let distance =
        view_height / (2.0 * libm::tan(field_of_view.to_radians() / 2.0)) + half_diagonal;
    checked_at(
        Camera {
            projection: camera.projection,
            eye: add(fitted.target, scaled(frame.back, distance)),
            ..fitted
        },
        width,
        height,
    )
}

/// Whether a renderer draws `camera`'s view of bodies in `boxes` in a pane `rows` pixels
/// high: any perspective view; an orthographic one no smaller than the renderer's own least
/// there for its bodies' reach from its target (`rows × reach / 2^19`).
pub fn drawable(
    camera: &Camera,
    rows: u32,
    boxes: impl IntoIterator<Item = [[f64; 3]; 2]>,
) -> bool {
    match camera.projection {
        Projection::Perspective { .. } => true,
        Projection::Orthographic { view_height } => {
            view_height >= least_view_height(rows, views::reach(camera.target, boxes))
        }
    }
}

/// `camera` with its orthographic view raised, if it's under it, to the least the bodies in
/// `boxes` allow from its target in a pane `rows` pixels high (`views::least_view_height`:
/// the renderer's least there and a hundredth more), so every view the viewport takes is
/// [`drawable`] and a control can always move it on. A perspective view as it is.
pub fn clamped(
    camera: Camera,
    rows: u32,
    boxes: impl IntoIterator<Item = [[f64; 3]; 2]>,
) -> Camera {
    let Projection::Orthographic { view_height } = camera.projection else {
        return camera;
    };
    let least = views::least_view_height(rows, views::reach(camera.target, boxes));
    if view_height >= least {
        return camera;
    }
    Camera {
        projection: Projection::Orthographic { view_height: least },
        ..camera
    }
}

/// The named view `view` of `bounds` (the ViewCube's faces and corners, and Home): the
/// core's own camera for it, which `render` and `meshes` fit too, so the app and an agent
/// asking for the view see the same.
pub fn named(view: NamedView, bounds: [[f64; 3]; 2], width: u32, height: u32) -> Option<Camera> {
    named_in(view, bounds, width, height, Inset::default())
}

/// The bands of a pane, in pixels in from each side, that panels over the canvas cover
/// (sketch mode's bar at the top and its palette at the right): a view fitted with them
/// fits its box into the rest of the pane, centred there. None (the default): the whole
/// pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inset {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

/// What sketch mode adds to a view's fit (the M5.12 slice's finding, docs/walkthrough/m5.md):
/// the open sketch's geometry, taken into the bounds with the bodies'
/// (`crate::sketch::bounds`), and the bands its bar and palette cover, which a fitted view
/// keeps its box out of. The tab sets it on its view each frame; the default, outside sketch
/// mode, adds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Framing {
    /// The open sketch's geometry, as a box in model space.
    pub sketch_box: Option<[[f64; 3]; 2]>,
    pub inset: Inset,
}

impl Inset {
    /// The part of a `width` by `height` pane it leaves, if it leaves at least a pixel each
    /// way: its width and height.
    fn free(self, width: u32, height: u32) -> Option<(u32, u32)> {
        let across = width.checked_sub(self.left)?.checked_sub(self.right)?;
        let down = height.checked_sub(self.top)?.checked_sub(self.bottom)?;
        (across > 0 && down > 0).then_some((across, down))
    }
}

/// The view of a box `make` fits, orthographic, for the part of a `width` by `height` pane
/// that `inset` leaves: made at that part's size, then made the whole pane's, as many
/// millimetres a pixel (so as high as the pane at that scale) and moved so its target shows
/// at that part's centre. With no inset, `make`'s camera for the whole pane, as it is; with
/// one that leaves no pixel, the same, as if there were none. Not checked: its caller
/// checks the camera it gives.
fn inset_fit(
    width: u32,
    height: u32,
    inset: Inset,
    make: impl Fn(u32, u32) -> Camera,
) -> Option<Camera> {
    let Some((across, down)) = inset
        .free(width, height)
        .filter(|_| inset != Inset::default())
    else {
        return Some(make(width, height));
    };
    let fitted = make(across, down);
    let Projection::Orthographic { view_height } = fitted.projection else {
        return None;
    };
    let frame = fitted.frame().ok()?;
    let mm = view_height / f64::from(down);
    // The part's centre is right of the pane's by half the left band less half the right,
    // and below it (the screen's y down) by half the top band less half the bottom: the
    // pane's centre, where the target shows, is that far left of and above the box's.
    let right = (f64::from(inset.left) - f64::from(inset.right)) / 2.0 * mm;
    let below = (f64::from(inset.top) - f64::from(inset.bottom)) / 2.0 * mm;
    let offset = add(scaled(frame.right, -right), scaled(frame.up, below));
    Some(Camera {
        projection: Projection::Orthographic {
            view_height: mm * f64::from(height),
        },
        eye: add(fitted.eye, offset),
        target: add(fitted.target, offset),
        up: fitted.up,
    })
}

/// [`named`], fitted into the part of the pane `inset` leaves.
pub fn named_in(
    view: NamedView,
    bounds: [[f64; 3]; 2],
    width: u32,
    height: u32,
    inset: Inset,
) -> Option<Camera> {
    look_at(view.direction(), view.up(), bounds, width, height, inset)
}

/// Look At (Fusion's; SolidWorks' Normal To): the view square to a plane, looking at it
/// from `toward`, its outward normal (a unit vector from the plane toward the eye), with
/// `up` (a unit vector in the plane) up the image, fitted to `bounds` in the part of the
/// pane `inset` leaves, as a named view is.
pub fn look_at(
    toward: [f64; 3],
    up: [f64; 3],
    bounds: [[f64; 3]; 2],
    width: u32,
    height: u32,
    inset: Inset,
) -> Option<Camera> {
    let (width, height) = (width.max(1), height.max(1));
    let camera = inset_fit(width, height, inset, |across, down| {
        views::fit_along(toward, up, bounds, across, down).0
    })?;
    checked_at(camera, width, height)
}

/// Which way is up when looking at a face square from `toward` (its unit outward normal),
/// as the named views have it ([`NamedView::up`]): +z across the view, made a unit as the
/// core makes a corner's; or +y when the face looks along z, as `top` and `bottom` have it,
/// which is when +z's part across the view is under a millionth, too short to steer by.
pub fn up_across(toward: [f64; 3]) -> [f64; 3] {
    let z = [0.0, 0.0, 1.0];
    let across = sub(z, scaled(toward, dot(z, toward)));
    let length = dot(across, across).sqrt();
    if length < 1e-6 {
        return [0.0, 1.0, 0.0];
    }
    across.map(|part| part / length)
}

/// The view switched to a perspective one (Fusion's ViewCube menu, *Camera*), at
/// [`PERSPECTIVE_FIELD_OF_VIEW`], its eye moved along the view so the target's depth shows
/// as much as before; or to an orthographic one, as high as the perspective view is at the
/// target. Either way the same projection again changes nothing.
pub fn projected(camera: &Camera, perspective: bool) -> Option<Camera> {
    let frame = camera.frame().ok()?;
    match (camera.projection, perspective) {
        (Projection::Orthographic { view_height }, true) => {
            let half = libm::tan(PERSPECTIVE_FIELD_OF_VIEW.to_radians() / 2.0);
            let distance = view_height / (2.0 * half);
            checked(Camera {
                projection: Projection::Perspective {
                    field_of_view: PERSPECTIVE_FIELD_OF_VIEW,
                },
                eye: add(camera.target, scaled(frame.back, distance)),
                ..*camera
            })
        }
        (Projection::Perspective { field_of_view }, false) => {
            let view_height = 2.0 * frame.distance * libm::tan(field_of_view.to_radians() / 2.0);
            checked(Camera {
                projection: Projection::Orthographic { view_height },
                ..*camera
            })
        }
        _ => Some(camera.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From −y onto the origin, 100 mm away, 80 mm high.
    fn front() -> Camera {
        Camera {
            projection: Projection::Orthographic { view_height: 80.0 },
            eye: [0.0, -100.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    #[track_caller]
    fn near(a: [f64; 3], b: [f64; 3], within: f64) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() <= within, "{a:?} against {b:?}");
        }
    }

    fn height(camera: &Camera) -> f64 {
        match camera.projection {
            Projection::Orthographic { view_height } => view_height,
            Projection::Perspective { .. } => f64::NAN,
        }
    }

    #[test]
    fn a_drag_of_180_points_across_turns_a_quarter_about_z_bringing_the_left_side_into_view() {
        // 180 points at half a degree each: 90°. Turned with the pointer to the right, the
        // model shows its left: the eye goes from −y to −x, the target and up kept.
        let turned = orbit(&front(), 180.0, 0.0).unwrap();
        near(turned.eye, [-100.0, 0.0, 0.0], 1e-12);
        assert_eq!(turned.target, [0.0, 0.0, 0.0]);
        near(turned.up, [0.0, 0.0, 1.0], 1e-15);
        // Back by as much, the other way: the front again.
        let back = orbit(&turned, -180.0, 0.0).unwrap();
        near(back.eye, [0.0, -100.0, 0.0], 1e-12);
    }

    #[test]
    fn a_drag_of_180_points_up_turns_a_quarter_about_the_views_right_showing_the_bottom() {
        // Dragged up (dy < 0): the model's front turns up, so the eye goes under it, and the
        // image's up is the model's front, −y.
        let turned = orbit(&front(), 0.0, -180.0).unwrap();
        near(turned.eye, [0.0, 0.0, -100.0], 1e-12);
        near(turned.up, [0.0, -1.0, 0.0], 1e-15);
        // 60 points down instead: 30° the other way, the eye up at (0, −100 cos 30°,
        // 100 sin 30°) = (0, −86.6025…, 50).
        let down = orbit(&front(), 0.0, 60.0).unwrap();
        near(down.eye, [0.0, -86.602_540_378_443_86, 50.0], 1e-12);
    }

    #[test]
    fn an_orbit_turns_about_the_target_however_far_it_is_from_the_origin() {
        // 5·10⁴ mm out: the target is kept to the bit, the eye's distance from it to well
        // under a nanometre, and a turn and its inverse come back to within 1e-9 mm.
        let far = Camera {
            eye: [50_000.0, 49_900.0, 50_000.0],
            target: [50_000.0, 50_000.0, 50_000.0],
            ..front()
        };
        let turned = orbit(&far, 37.0, -23.0).unwrap();
        assert_eq!(turned.target, far.target);
        let d = sub(turned.eye, turned.target);
        assert!((dot(d, d).sqrt() - 100.0).abs() < 1e-9);
        let back = orbit(&orbit(&turned, 0.0, 23.0).unwrap(), -37.0, 0.0).unwrap();
        near(back.eye, far.eye, 1e-9);
    }

    #[test]
    fn a_pan_moves_the_eye_and_target_by_the_points_dragged_at_the_targets_scale() {
        // 80 mm over 400 points: 0.2 mm a point. Dragged 50 right and 25 down, the model
        // follows, so the view moves 10 mm left (−x, the front view's right being +x) and
        // 5 mm up (+z).
        let panned = pan(&front(), 50.0, 25.0, 400.0).unwrap();
        near(panned.eye, [-10.0, -100.0, 5.0], 1e-12);
        near(panned.target, [-10.0, 0.0, 5.0], 1e-12);
        assert_eq!(height(&panned), 80.0);
        // In perspective, the scale is the target's: 2 · 100 · tan 15° / 400 mm a point.
        let perspective = Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            ..front()
        };
        let mm = 2.0 * 100.0 * libm::tan(15f64.to_radians()) / 400.0;
        let panned = pan(&perspective, 50.0, 0.0, 400.0).unwrap();
        near(panned.target, [-50.0 * mm, 0.0, 0.0], 1e-12);
    }

    #[test]
    fn a_zoom_keeps_the_point_under_the_cursor_where_it_is() {
        // 200 points of wheel: the view halves, 80 to 40 mm. The cursor 100 points right
        // of centre and 50 up, at 0.2 mm a point over 400: the point (20, 0, 10) mm. Halved
        // about it, the target goes from the origin to (10, 0, 5), the eye with it.
        let zoomed = zoom(&front(), 200.0, [100.0, -50.0], 400.0).unwrap();
        assert_eq!(height(&zoomed), 40.0);
        near(zoomed.target, [10.0, 0.0, 5.0], 1e-12);
        near(zoomed.eye, [10.0, -100.0, 5.0], 1e-12);
        // That point is still 100 points right and 50 up: (20 − 10) / 0.1 and (10 − 5) / 0.1.
        // Out again by as much, from the same place, the view is the first.
        let out = zoom(&zoomed, -200.0, [100.0, -50.0], 400.0).unwrap();
        assert_eq!(height(&out), 80.0);
        near(out.target, [0.0, 0.0, 0.0], 1e-12);
        // At the centre, the target stays.
        let centred = zoom(&front(), 200.0, [0.0, 0.0], 400.0).unwrap();
        assert_eq!(centred.target, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_perspective_zoom_moves_the_eye_toward_the_point_under_the_cursor() {
        // Halved about the point under the cursor at the target's depth: the eye's distance
        // to the target halves, and that point stays on the eye's ray to it.
        let perspective = Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            ..front()
        };
        let mm = 2.0 * 100.0 * libm::tan(15f64.to_radians()) / 400.0;
        let under = [100.0 * mm, 0.0, 0.0];
        let zoomed = zoom(&perspective, 200.0, [100.0, 0.0], 400.0).unwrap();
        near(zoomed.eye, [under[0] / 2.0, -50.0, 0.0], 1e-12);
        near(zoomed.target, [under[0] / 2.0, 0.0, 0.0], 1e-12);
    }

    #[test]
    fn a_zoom_past_the_least_view_height_a_camera_may_have_is_refused_and_the_view_kept() {
        // 1e-6 mm is the least view height: 80 mm halved 27 times is 6e-7 mm.
        assert!(zoom(&front(), 200.0 * 27.0, [0.0, 0.0], 400.0).is_none());
        assert!(zoom(&front(), 200.0 * 26.0, [0.0, 0.0], 400.0).is_some());
    }

    #[test]
    fn a_fit_centres_the_box_and_takes_a_tenth_more_than_its_extent() {
        // From the front, a box 0..40 by 0..20 by −1..11: across, 40 mm; up, 12 mm. The
        // image is 1,000 by 500, so 40 mm across is 20 mm of height: the view is 1.1 × 20 =
        // 22 mm high, centred on (20, 10, 5), its eye two half diagonals back, √(40² + 20² +
        // 12²) = 46.303…
        let fitted = fit(&front(), [[0.0, 0.0, -1.0], [40.0, 20.0, 11.0]], 1000, 500).unwrap();
        assert!((height(&fitted) - 22.0).abs() < 1e-12);
        near(fitted.target, [20.0, 10.0, 5.0], 1e-12);
        let diagonal = (40.0f64 * 40.0 + 20.0 * 20.0 + 12.0 * 12.0).sqrt();
        near(fitted.eye, [20.0, 10.0 - diagonal, 5.0], 1e-12);
        // Square: the 12 mm of height is no longer the lesser, 40 mm across is: 44 mm.
        let square = fit(&front(), [[0.0, 0.0, -1.0], [40.0, 20.0, 11.0]], 500, 500).unwrap();
        assert!((height(&square) - 44.0).abs() < 1e-12);
    }

    #[test]
    fn a_perspective_fit_stands_back_so_the_box_centre_shows_the_fit_height() {
        // A 20 mm cube at the origin's corner, from the front, square: 22 mm high at its
        // centre, so 11 / tan 15° back from it, and half the diagonal (10√3) more.
        let perspective = Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            ..front()
        };
        let fitted = fit(&perspective, [[0.0; 3], [20.0; 3]], 500, 500).unwrap();
        let back = 11.0 / libm::tan(15f64.to_radians()) + 10.0 * 3f64.sqrt();
        near(fitted.eye, [10.0, 10.0 - back, 10.0], 1e-9);
        // Still perspective, at its own angle: a fit keeps the projection.
        assert_eq!(
            fitted.projection,
            Projection::Perspective {
                field_of_view: 30.0
            }
        );
    }

    #[test]
    fn a_point_spans_the_views_height_over_the_points_and_a_view_of_no_points_none() {
        // 80 mm over 400 points: 0.2 mm a point. Over no points, or a height that isn't a
        // number of points, there's no span: a pan of an empty pane moves nothing.
        assert_eq!(mm_per_point(&front(), 400.0), Some(0.2));
        assert_eq!(mm_per_point(&front(), 0.0), None);
        assert_eq!(mm_per_point(&front(), -400.0), None);
        assert_eq!(mm_per_point(&front(), f64::INFINITY), None);
        assert_eq!(mm_per_point(&front(), f64::NAN), None);
    }

    #[test]
    fn home_on_a_20_mm_cube_looks_from_its_corner_at_its_centre_a_tenth_over_its_extent() {
        // From (1, −1, 1)/√3 onto the centre (10, 10, 10), two half diagonals (2 · 10√3)
        // back: the eye at (30, −10, 30). Up, +z made perpendicular, (−1, 1, 2)/√6, spans
        // 80/√6 of the cube (from (20, 0, 0) to (0, 20, 20)); across, right (1, 1, 0)/√2
        // spans 40/√2, less: so 1.1 · 80/√6 = 35.926 high.
        let home = named(HOME, [[0.0; 3], [20.0; 3]], 400, 400).unwrap();
        near(home.target, [10.0; 3], 1e-12);
        near(home.eye, [30.0, -10.0, 30.0], 1e-12);
        let six = 6f64.sqrt();
        near(home.up, [-1.0 / six, 1.0 / six, 2.0 / six], 1e-15);
        assert!((height(&home) - 1.1 * 80.0 / six).abs() < 1e-12);
        assert!((height(&home) - 35.926).abs() < 5e-4);
        // F6 from Home's direction fits the same: its extent over all eight corners (a fit
        // of two corners alone, (0, 0, 0) and (20, 20, 20), would span 40/√6 up and give
        // 31.11 by the right's 40/√2).
        let refit = fit(&home, [[0.0; 3], [20.0; 3]], 400, 400).unwrap();
        assert!((height(&refit) - 1.1 * 80.0 / six).abs() < 1e-12);
    }

    #[test]
    fn a_needle_seen_end_on_is_fitted_no_smaller_than_its_reach_allows() {
        // 1 by 500,000 by 1 mm, from the front: 1 mm across, so 1.1 mm by its extent; but
        // its half diagonal, 250,000 mm, makes the least at 400 rows 400 · 250,000 / 2^19 ·
        // 1.01 = 192.6422 mm (by hand).
        let needle = [[0.0; 3], [1.0, 500_000.0, 1.0]];
        let fitted = fit(&front(), needle, 400, 400).unwrap();
        assert!(
            (height(&fitted) - 192.642_211_9).abs() < 1e-6,
            "{}",
            height(&fitted)
        );
        assert!(drawable(&fitted, 400, [needle]));
    }

    #[test]
    fn an_orthographic_view_smaller_than_the_renderers_least_isnt_drawable_and_is_clamped_to_ours()
    {
        // The block (0, 0, 0) to (40, 20, 10) from the origin reaches √2100 = 45.83 mm: at
        // 400 rows the renderer draws down to 400 · 45.83 / 2^19 = 0.034962 mm; a clamp
        // raises a view to a hundredth over that, 0.035312 mm.
        let block = [[0.0, 0.0, 0.0], [40.0, 20.0, 10.0]];
        let renderers = 2100f64.sqrt() * 400.0 / 524_288.0;
        assert!((renderers - 0.034_962_28).abs() < 1e-8, "{renderers}");
        let ours = renderers * 1.01;
        let at = |view_height: f64| Camera {
            projection: Projection::Orthographic { view_height },
            ..front()
        };
        assert!(drawable(&at(renderers * 1.000_001), 400, [block]));
        assert!(!drawable(&at(renderers * 0.999_999), 400, [block]));
        // Between the two: drawn, and a clamp leaves it.
        assert!(drawable(&at(renderers * 1.005), 400, [block]));
        for under in [1e-6, renderers, ours * 0.999] {
            assert!((height(&clamped(at(under), 400, [block])) - ours).abs() < 1e-18);
        }
        assert_eq!(clamped(at(80.0), 400, [block]), at(80.0));
        // Twice the rows, twice the least.
        assert!((height(&clamped(at(1e-6), 800, [block])) - 2.0 * ours).abs() < 1e-15);
        // With no bodies, anything the camera's check takes; in perspective, anything.
        assert!(drawable(&at(1e-6), 400, []));
        assert_eq!(clamped(at(1e-6), 400, []), at(1e-6));
        let perspective = Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            ..front()
        };
        assert!(drawable(&perspective, 400, [block]));
        assert_eq!(clamped(perspective.clone(), 400, [block]), perspective);
    }

    #[test]
    fn a_needle_seen_end_on_fitted_by_f6_or_home_is_drawn() {
        // Its fit is exactly its least by the half diagonal (with a hundredth over the
        // renderer's): an ulp of the reach mustn't make it refused.
        let needle = [[0.0; 3], [1.0, 500_000.0, 1.0]];
        let fitted = fit(&front(), needle, 400, 400).unwrap();
        assert!(drawable(&fitted, 400, [needle]));
        let home = named(NamedView::Front, needle, 400, 400).unwrap();
        assert!(drawable(&home, 400, [needle]));
    }

    #[test]
    fn a_switch_to_perspective_keeps_the_targets_view_and_back_again_the_height() {
        // 80 mm high at 30°: the eye 40 / tan 15° = 149.282… back.
        let perspective = projected(&front(), true).unwrap();
        let back = 40.0 / libm::tan(15f64.to_radians());
        near(perspective.eye, [0.0, -back, 0.0], 1e-12);
        let ortho = projected(&perspective, false).unwrap();
        assert!((height(&ortho) - 80.0).abs() < 1e-12);
        assert_eq!(projected(&front(), false).unwrap(), front());
    }

    /// The slice's L on XZ: 60 along x, 50 up z.
    const L: [[f64; 3]; 2] = [[0.0, 0.0, 0.0], [60.0, 0.0, 50.0]];

    /// A 1000 by 800 pane with a 100-pixel band over its top and a 200-pixel one over its
    /// right: the 800 by 700 left below them.
    const BANDS: Inset = Inset {
        top: 100,
        right: 200,
        bottom: 0,
        left: 0,
    };

    #[test]
    fn a_front_view_fitted_below_and_beside_bands_centres_its_box_in_the_rest_at_its_scale() {
        // At 800 by 700, the core's fit: 50 up, 60 across × 700 / 800 = 52.5, so 1.1 × 52.5
        // = 57.75 mm high over 700 pixels, 0.0825 mm a pixel; the pane's 800 rows are 66 mm.
        // The rest's centre, (400, 450), is 100 pixels left of the pane's (500, 400) and 50
        // below it: the target is the box's centre, (30, 0, 25), moved 100 × 0.0825 = 8.25
        // right (+x) and 50 × 0.0825 = 4.125 up (+z). The eye stays two half diagonals
        // (2 × √6100 / 2 = 78.102…) out along −y.
        let fitted = named_in(NamedView::Front, L, 1000, 800, BANDS).unwrap();
        assert!((height(&fitted) - 66.0).abs() < 1e-12, "{fitted:?}");
        near(fitted.target, [38.25, 0.0, 29.125], 1e-12);
        near(fitted.eye, [38.25, -6100f64.sqrt(), 29.125], 1e-12);
        assert_eq!(fitted.up, [0.0, 0.0, 1.0]);
        // So the L's top, z = 50, shows 400 − (50 − 29.125) / 0.0825 = 146.97 rows down,
        // under the top band's 100; its left, x = 0, at 500 − 38.25 / 0.0825 = 36.36, and
        // its right, x = 60, at 763.64, left of the right band at 800.
        let mm = 66.0 / 800.0;
        assert!(400.0 - (50.0 - fitted.target[2]) / mm > 100.0);
        assert!(500.0 + (60.0 - fitted.target[0]) / mm < 800.0);
        // `fit` looking the same way gives the same view.
        let refit = fit_in(&front(), L, 1000, 800, BANDS).unwrap();
        assert!((height(&refit) - 66.0).abs() < 1e-12);
        near(refit.target, [38.25, 0.0, 29.125], 1e-12);
    }

    #[test]
    fn bands_at_the_left_and_bottom_move_the_box_right_and_up_into_the_rest() {
        // As BANDS' test, the rest 800 by 700 at 0.0825 mm a pixel, but its centre, (600,
        // 350), is 100 right of the pane's (500, 400) and 50 above it: the target moves 8.25
        // left (−x) and 4.125 down (−z) of the box's centre, (30, 0, 25).
        let bands = Inset {
            top: 0,
            right: 0,
            bottom: 100,
            left: 200,
        };
        let fitted = named_in(NamedView::Front, L, 1000, 800, bands).unwrap();
        assert!((height(&fitted) - 66.0).abs() < 1e-12, "{fitted:?}");
        near(fitted.target, [21.75, 0.0, 20.875], 1e-12);
    }

    #[test]
    fn a_band_leaving_one_pixel_across_fits_the_box_into_that_pixel() {
        // 999 of 1000 columns covered at the right: the rest is 1 by 800, so the core's fit
        // takes 60 across × 800 / 1 = 48 000, and 1.1 × that is 52 800 mm over 800 rows, 66 mm
        // a pixel. The rest's centre, half a pixel in from the pane's left, is 499.5 pixels
        // left of the pane's: the target is 499.5 × 66 = 32 967 right (+x) of the box's
        // centre, at the box's middle height.
        let sliver = Inset {
            right: 999,
            ..Inset::default()
        };
        let fitted = named_in(NamedView::Front, L, 1000, 800, sliver).unwrap();
        assert!((height(&fitted) - 52_800.0).abs() < 1e-6, "{fitted:?}");
        near(fitted.target, [30.0 + 32_967.0, 0.0, 25.0], 1e-6);
    }

    #[test]
    fn with_no_bands_or_bands_that_leave_no_pixel_a_fit_is_the_whole_panes() {
        let whole = named(NamedView::Front, L, 1000, 800).unwrap();
        // The core's own fit, at the whole pane: 1.1 × max(50, 60 × 800 / 1000) = 55.
        assert!((height(&whole) - 55.0).abs() < 1e-12);
        near(whole.target, [30.0, 0.0, 25.0], 1e-12);
        let covered = Inset {
            top: 800,
            ..Inset::default()
        };
        assert_eq!(
            named_in(NamedView::Front, L, 1000, 800, covered),
            Some(whole.clone())
        );
        let past = Inset {
            right: 600,
            left: 600,
            ..Inset::default()
        };
        assert_eq!(
            named_in(NamedView::Front, L, 1000, 800, past),
            Some(whole.clone())
        );
        // Bands that meet: 500 from each side of 1000 leave no column, as none.
        let meeting = Inset {
            right: 500,
            left: 500,
            ..Inset::default()
        };
        assert_eq!(
            named_in(NamedView::Front, L, 1000, 800, meeting),
            Some(whole)
        );
    }

    #[test]
    fn a_perspective_view_fitted_with_bands_keeps_its_angle_and_the_target_the_bands_give() {
        let perspective = projected(&front(), true).unwrap();
        let fitted = fit_in(&perspective, L, 1000, 800, BANDS).unwrap();
        assert_eq!(
            fitted.projection,
            Projection::Perspective {
                field_of_view: PERSPECTIVE_FIELD_OF_VIEW
            }
        );
        near(fitted.target, [38.25, 0.0, 29.125], 1e-12);
        // 66 mm high at the target at 30°, and half the diagonal more: 33 / tan 15° +
        // √6100 / 2 back along −y.
        let back = 33.0 / libm::tan(15f64.to_radians()) + 6100f64.sqrt() / 2.0;
        near(fitted.eye, [38.25, -back, 29.125], 1e-9);
    }

    #[test]
    fn look_at_a_face_square_from_its_normal_fits_its_box_with_z_up_the_image() {
        // A unit cube seen square to a face facing (1, 1, 0) / √2, +z up: the image's right
        // is up × toward = (−1, 1, 0) / √2, so the corners span 1 up and √2 across; at 100
        // by 100, 1.1 × √2 mm high. The eye is two half diagonals (√3) out from the centre.
        let toward = [1.0 / 2f64.sqrt(), 1.0 / 2f64.sqrt(), 0.0];
        let cube = [[0.0; 3], [1.0; 3]];
        let looked = look_at(toward, [0.0, 0.0, 1.0], cube, 100, 100, Inset::default()).unwrap();
        assert!((height(&looked) - 1.1 * 2f64.sqrt()).abs() < 1e-12);
        near(looked.target, [0.5, 0.5, 0.5], 1e-15);
        let out = 3f64.sqrt() / 2f64.sqrt();
        near(looked.eye, [0.5 + out, 0.5 + out, 0.5], 1e-12);
        assert_eq!(looked.up, [0.0, 0.0, 1.0]);
        // From −y with +z up, the front view.
        assert_eq!(
            look_at([0.0, -1.0, 0.0], [0.0, 0.0, 1.0], L, 1000, 800, BANDS),
            named_in(NamedView::Front, L, 1000, 800, BANDS)
        );
    }

    #[test]
    fn up_across_a_face_is_z_made_square_to_its_normal_or_y_for_a_face_along_z() {
        assert_eq!(up_across([0.0, 0.0, 1.0]), [0.0, 1.0, 0.0]);
        assert_eq!(up_across([0.0, 0.0, -1.0]), [0.0, 1.0, 0.0]);
        assert_eq!(up_across([1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]);
        // (1, 1, 1) / √3: z less its part along it, (−1/3, −1/3, 2/3), made a unit:
        // (−1, −1, 2) / √6.
        let corner = [1.0 / 3f64.sqrt(); 3];
        let s = 6f64.sqrt();
        near(up_across(corner), [-1.0 / s, -1.0 / s, 2.0 / s], 1e-15);
        // Within a millionth of z, still z's; a face a hundredth off it turns z with it.
        assert_eq!(up_across([0.0, 1e-7, 1.0]), [0.0, 1.0, 0.0]);
        // At exactly a millionth (from (0, 1e-6, 1), +z's part across is (0, −1e-6, 0), its
        // length 1e-6 to the bit, as √(x²) is x), it steers by that part: −y, not +y.
        assert_eq!(up_across([0.0, 1e-6, 1.0]), [0.0, -1.0, 0.0]);
        let tipped = [0.0, -0.01, (1.0f64 - 1e-4).sqrt()];
        let up = up_across(tipped);
        assert!(up[1] > 0.99 && up[2] > 0.0, "{up:?}");
    }
}
