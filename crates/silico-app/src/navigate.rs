//! The viewport's camera controls (docs/design/m5.0-app.md, section 8, *Navigation*;
//! docs/ux/fusion.md, *Navigation defaults* and *ViewCube and Navigation Bar*): orbit about
//! the target, pan, zoom toward the cursor, fit, the named views, and the projection. The
//! camera is the app's own (decision 29), held in `f64` as the core's [`Camera`], and never
//! reaches a document (section 11). Each control gives the camera it makes, or none when
//! that camera fails [`Camera::check`] (non-finite, or past the least or the most a camera
//! may be), so the view stays as it was; [`drawable`] says whether a renderer draws a
//! camera's view of the bodies (an orthographic view no smaller than their reach allows),
//! which the viewport asks of every camera a control makes.

use silico_core::views::{self, NamedView};
use silico_protocol::vec3::{cross, dot, sub};
use silico_protocol::{Camera, Projection};

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

/// `camera` if it passes its check.
fn checked(camera: Camera) -> Option<Camera> {
    camera.check().ok().map(|()| camera)
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
    let frame = camera.frame().ok()?;
    let (fitted, _) = views::fit_along(frame.back, frame.up, bounds, width, height);
    let Projection::Perspective { field_of_view } = camera.projection else {
        return checked(fitted);
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
    checked(Camera {
        projection: camera.projection,
        eye: add(fitted.target, scaled(frame.back, distance)),
        ..fitted
    })
}

/// Whether a renderer draws `camera`'s view of bodies in `boxes`: any perspective view; an
/// orthographic one no smaller than the least its bodies' reach from its target allows
/// (`views::least_view_height`, which fits keep to as well).
pub fn drawable(camera: &Camera, boxes: impl IntoIterator<Item = [[f64; 3]; 2]>) -> bool {
    match camera.projection {
        Projection::Perspective { .. } => true,
        Projection::Orthographic { view_height } => {
            view_height >= views::least_view_height(views::reach(camera.target, boxes))
        }
    }
}

/// The named view `view` of `bounds` (the ViewCube's faces and corners, and Home): the
/// core's own camera for it, which `render` and `meshes` fit too, so the app and an agent
/// asking for the view see the same.
pub fn named(view: NamedView, bounds: [[f64; 3]; 2], width: u32, height: u32) -> Option<Camera> {
    checked(view.camera(bounds, width.max(1), height.max(1)))
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
        // its half diagonal, 250,000 mm, makes the least 250,000 · 1e-5 · 1.01 = 2.525 mm.
        let needle = [[0.0; 3], [1.0, 500_000.0, 1.0]];
        let fitted = fit(&front(), needle, 400, 400).unwrap();
        assert!(
            (height(&fitted) - 2.525).abs() < 1e-6,
            "{}",
            height(&fitted)
        );
        assert!(drawable(&fitted, [needle]));
    }

    #[test]
    fn an_orthographic_view_smaller_than_its_bodies_reach_allows_isnt_drawable() {
        // The block (0, 0, 0) to (40, 20, 10) from the origin reaches √2100 = 45.83 mm: the
        // least is 45.83 · 1e-5 · 1.01 = 4.6286e-4 mm.
        let block = [[0.0, 0.0, 0.0], [40.0, 20.0, 10.0]];
        let least = 2100f64.sqrt() * 1e-5 * 1.01;
        let at = |view_height: f64| Camera {
            projection: Projection::Orthographic { view_height },
            ..front()
        };
        assert!(drawable(&at(least * 1.000_001), [block]));
        assert!(!drawable(&at(least * 0.999_999), [block]));
        // With no bodies, anything the camera's check takes; in perspective, anything.
        assert!(drawable(&at(1e-6), []));
        let perspective = Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            ..front()
        };
        assert!(drawable(&perspective, [block]));
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
}
