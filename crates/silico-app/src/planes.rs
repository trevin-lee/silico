//! The origin planes drawn in the viewport (the approved prototype's `PLANES`,
//! `buildPlanes`, and `VP.setPlanes`, docs/design/m5-ui/js/viewport.js; its hover,
//! interact.js's `startPickPlane` and `planeRow`). Each of XY, XZ, and YZ is the square from
//! (−4, −4) to (46, 46) in its own (U, V) (XY's U is x and V y; XZ's, x and z; YZ's, y and z),
//! filled in [`theme::VIEWPORT_PLANE_FILL`] and outlined in
//! [`theme::VIEWPORT_PLANE_OUTLINE`], [`theme::VIEWPORT_PLANE_WIDTH`] wide; a plane hovered
//! in [`theme::VIEWPORT_PLANE_HOVER_FILL`] and [`theme::VIEWPORT_PLANE_HOVER_OUTLINE`].
//! The renderer draws them among the bodies, translucent, and in no picking
//! (`silico_render::render::Plane`).
//!
//! Which are drawn, one rule for every way a plane shows ([`drawn`]):
//! - while Create Sketch's pick is on (`crate::plane_pick`), all three, the one under the
//!   pointer hovered: the prototype's `startPickPlane` and `VP.setPlanes({ XY: true, XZ:
//!   true, YZ: true }, hp)`;
//! - else each whose browser row's eye is open (each opens hidden, as the prototype's,
//!   Fusion's, and SolidWorks' origin planes do), and the one whose browser row the pointer
//!   is on, hovered, its eye open or not (the prototype's `planeRow` shows a plane its row
//!   selects, hidden or not, lit; the app's origin rows don't select, so the pointer on the
//!   row is what lights it, as a timeline item hovered lights its faces).
//!
//! Where the pointer's ray meets them is [`under`]'s: the pick's, as the prototype's
//! `VP.pickPlane` casts on the planes it shows.

use silico_core::document::OriginPlane;
use silico_protocol::{Camera, MIN_EYE_DISTANCE, Projection, vec3};
use silico_render::render::{Plane, ray};

use crate::theme;

/// The origin planes, each with its browser row's name (`crate::tab::Hideable::Origin`),
/// in the browser's order.
pub const ORIGIN_PLANES: [(OriginPlane, &str); 3] = [
    (OriginPlane::Xy, "XY"),
    (OriginPlane::Xz, "XZ"),
    (OriginPlane::Yz, "YZ"),
];

/// The prototype's square on each plane, in its (U, V): from (−4, −4) to (46, 46).
const LOW: f64 = -4.0;
const HIGH: f64 = 46.0;

/// The origin plane whose browser row is named `name`, if one is.
pub fn of_row(name: &str) -> Option<OriginPlane> {
    ORIGIN_PLANES
        .iter()
        .find(|(_, row)| *row == name)
        .map(|(plane, _)| *plane)
}

/// The browser row's name of `plane`.
pub fn row_of(plane: OriginPlane) -> &'static str {
    match plane {
        OriginPlane::Xy => "XY",
        OriginPlane::Xz => "XZ",
        OriginPlane::Yz => "YZ",
    }
}

/// `plane`'s square as the renderer draws it, in the hover's colours if `hovered`.
pub fn plane(plane: OriginPlane, hovered: bool) -> Plane {
    let (u, v) = match plane {
        OriginPlane::Xy => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        OriginPlane::Xz => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        OriginPlane::Yz => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    };
    let side = HIGH - LOW;
    let (fill, outline) = if hovered {
        (
            theme::VIEWPORT_PLANE_HOVER_FILL,
            theme::VIEWPORT_PLANE_HOVER_OUTLINE,
        )
    } else {
        (theme::VIEWPORT_PLANE_FILL, theme::VIEWPORT_PLANE_OUTLINE)
    };
    Plane {
        origin: std::array::from_fn(|k| LOW * (u[k] + v[k])),
        u: u.map(|c| c * side),
        v: v.map(|c| c * side),
        fill,
        outline,
        width: theme::VIEWPORT_PLANE_WIDTH,
    }
}

/// A plane a frame would draw, and whether its eye shows it: one its eye shows is in the
/// reach the view is clamped by, as a shown sketch's lines are; one shown for a moment
/// alone (its row hovered, Create Sketch's pick on) isn't, so it never zooms the view out,
/// and is drawn only where the view as it is holds it (`crate::viewport`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drawn {
    pub of: OriginPlane,
    pub plane: Plane,
    pub shown: bool,
}

/// The planes a frame draws, in [`ORIGIN_PLANES`]' order: each `shown` (by its eye) or
/// `hovered` (its browser row under the pointer), the hovered one in the hover's colours;
/// or, while Create Sketch's pick is on (`pick`, the plane under the pointer in the canvas,
/// if one is), all three, the one under the pointer hovered, in the canvas or else on its
/// row (whose click picks it, `crate::plane_pick`).
pub fn drawn(
    shown: impl Fn(OriginPlane) -> bool,
    hovered: Option<OriginPlane>,
    pick: Option<Option<OriginPlane>>,
) -> Vec<Drawn> {
    let (hovered, all) = match pick {
        Some(lit) => (lit.or(hovered), true),
        None => (hovered, false),
    };
    ORIGIN_PLANES
        .iter()
        .map(|(each, _)| (*each, shown(*each)))
        .filter(|(each, shown)| all || *shown || hovered == Some(*each))
        .map(|(each, shown)| Drawn {
            of: each,
            plane: plane(each, hovered == Some(each)),
            shown,
        })
        .collect()
}

/// The origin plane among `planes` (those a frame drew) that the ray through `pixel` meets
/// first, and how far along the ray, in mm, in a view from `camera` `size_px` in pixels
/// (`pixel` from the image's top left, in pixels, as [`silico_render::render::ray`] takes
/// it). Each plane is its square ([`plane`]), met where the ray crosses its plane inside it,
/// its sides included; one seen edge on is a line, and none of it is met. In perspective
/// nothing nearer the eye than [`MIN_EYE_DISTANCE`] along the view is met, as the drawing
/// cuts it there; orthographic, the whole line is (drawn in front of the eye too), so the
/// distance may be below 0. Two met at the same place along the ray (where two planes
/// cross) go by [`ORIGIN_PLANES`]' order. None where no plane is met, or where `ray` gives
/// no ray (a camera refused at this size, a pixel that isn't finite).
pub fn under(
    camera: &Camera,
    size_px: [u32; 2],
    pixel: [f64; 2],
    planes: &[OriginPlane],
) -> Option<(OriginPlane, f64)> {
    let (from, along) = ray(camera, size_px, pixel)?;
    let perspective = matches!(camera.projection, Projection::Perspective { .. });
    // `ray` gives a ray only for a camera its check takes, whose eye isn't its target.
    let view = vec3::normalize(vec3::sub(camera.target, camera.eye));
    let mut met: Option<(OriginPlane, f64)> = None;
    for (each, _) in ORIGIN_PLANES {
        if !planes.contains(&each) {
            continue;
        }
        let square = plane(each, false);
        let normal = vec3::cross(square.u, square.v);
        let across = vec3::dot(along, normal);
        // Seen edge on: drawn as a line, met nowhere.
        if across == 0.0 {
            continue;
        }
        let distance = vec3::dot(vec3::sub(square.origin, from), normal) / across;
        if !distance.is_finite() {
            continue;
        }
        let at: [f64; 3] = std::array::from_fn(|k| from[k] + distance * along[k]);
        if perspective && vec3::dot(vec3::sub(at, camera.eye), view) < MIN_EYE_DISTANCE {
            continue;
        }
        let off = vec3::sub(at, square.origin);
        let s = vec3::dot(off, square.u) / vec3::dot(square.u, square.u);
        let t = vec3::dot(off, square.v) / vec3::dot(square.v, square.v);
        if !((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t)) {
            continue;
        }
        if met.is_none_or(|(_, nearest)| distance < nearest) {
            met = Some((each, distance));
        }
    }
    met
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_plane_is_the_prototypes_square_from_minus_4_to_46_in_its_own_u_and_v() {
        // viewport.js's PLANES and `Geo.rectShape(-4, -4, 46, 46)`, worked by hand: XY's
        // corners (−4, −4, 0), (46, −4, 0), (46, 46, 0), (−4, 46, 0); XZ's in x and z; YZ's
        // in y and z.
        let corners = |p: OriginPlane| plane(p, false).corners();
        assert_eq!(
            corners(OriginPlane::Xy),
            [
                [-4.0, -4.0, 0.0],
                [46.0, -4.0, 0.0],
                [46.0, 46.0, 0.0],
                [-4.0, 46.0, 0.0]
            ]
        );
        assert_eq!(
            corners(OriginPlane::Xz),
            [
                [-4.0, 0.0, -4.0],
                [46.0, 0.0, -4.0],
                [46.0, 0.0, 46.0],
                [-4.0, 0.0, 46.0]
            ]
        );
        assert_eq!(
            corners(OriginPlane::Yz),
            [
                [0.0, -4.0, -4.0],
                [0.0, 46.0, -4.0],
                [0.0, 46.0, 46.0],
                [0.0, -4.0, 46.0]
            ]
        );
    }

    #[test]
    fn a_plane_is_the_prototypes_grey_and_one_hovered_its_blue() {
        // `#9f9f9f` at 0.08 (20 of 255) filled, at 0.6 (153) outlined, 1 px; hovered
        // `#186add` at 0.32 (82), outlined `#4676ff` at 0.6.
        let plain = plane(OriginPlane::Xy, false);
        assert_eq!(plain.fill, [0x9f, 0x9f, 0x9f, 20]);
        assert_eq!(plain.outline, [0x9f, 0x9f, 0x9f, 153]);
        assert_eq!(plain.width, 1.0);
        let lit = plane(OriginPlane::Xy, true);
        assert_eq!(lit.fill, [0x18, 0x6a, 0xdd, 82]);
        assert_eq!(lit.outline, [0x46, 0x76, 0xff, 153]);
        assert_eq!(lit.width, 1.0);
    }

    #[test]
    fn the_planes_drawn_are_those_shown_and_the_one_hovered_or_all_three_for_the_pick() {
        use OriginPlane::{Xy, Xz, Yz};
        let none = |_: OriginPlane| false;
        // Each plane, lit or not, and whether its eye shows it.
        let d = |p, lit, shown| Drawn {
            of: p,
            plane: plane(p, lit),
            shown,
        };
        assert!(drawn(none, None, None).is_empty());
        // XZ's eye open: XZ alone, plain, shown.
        assert_eq!(drawn(|p| p == Xz, None, None), [d(Xz, false, true)]);
        // YZ's row hovered, its eye closed: YZ drawn, lit, for the moment, beside XZ.
        assert_eq!(
            drawn(|p| p == Xz, Some(Yz), None),
            [d(Xz, false, true), d(Yz, true, false)]
        );
        // XZ hovered and shown: once, lit, shown.
        assert_eq!(drawn(|p| p == Xz, Some(Xz), None), [d(Xz, true, true)]);
        // The pick on, no plane under the pointer: all three, plain but YZ, whose row is
        // under it, each shown only by its eye.
        assert_eq!(
            drawn(none, Some(Yz), Some(None)),
            [d(Xy, false, false), d(Xz, false, false), d(Yz, true, false)]
        );
        // Nor a row: all three plain.
        assert_eq!(
            drawn(none, None, Some(None)),
            [
                d(Xy, false, false),
                d(Xz, false, false),
                d(Yz, false, false)
            ]
        );
        // One under the pointer in the canvas lit, not the row's.
        assert_eq!(
            drawn(none, Some(Yz), Some(Some(Xz))),
            [d(Xy, false, false), d(Xz, true, false), d(Yz, false, false)]
        );
        // The pick's XY under the pointer, YZ's eye open.
        assert_eq!(
            drawn(|p| p == Yz, None, Some(Some(Xy))),
            [d(Xy, true, false), d(Xz, false, false), d(Yz, false, true)]
        );
    }

    /// A camera from `eye` onto `target`, `projection`, z up (y up looking along z).
    fn camera(projection: Projection, eye: [f64; 3], target: [f64; 3]) -> Camera {
        let up = if eye[0] == target[0] && eye[1] == target[1] {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        Camera {
            projection,
            eye,
            target,
            up,
        }
    }

    const ALL: [OriginPlane; 3] = [OriginPlane::Xy, OriginPlane::Xz, OriginPlane::Yz];

    #[track_caller]
    fn met(got: Option<(OriginPlane, f64)>, plane: OriginPlane, distance: f64) {
        let (at, along) = got.unwrap();
        assert_eq!(at, plane);
        assert!(
            (along - distance).abs() < 1e-9,
            "{along} against {distance}"
        );
    }

    #[test]
    fn the_pointers_ray_meets_the_nearest_plane_drawn_inside_its_square() {
        use OriginPlane::{Xy, Xz, Yz};
        // In perspective from (60, −50, 10) onto (10, 0, 10), 100 pixels square: the
        // image's centre, (50, 50), looks along (−1, 1, 0)/√2, which meets XZ (y = 0) at
        // (10, 0, 10), inside its square (x and z −4 to 46), 50√2 from the eye; then YZ
        // (x = 0) at (0, 10, 10), inside its, 60√2; and XY (z = 0) never: the ray runs
        // along it. All by hand.
        let side = camera(
            Projection::Perspective {
                field_of_view: 45.0,
            },
            [60.0, -50.0, 10.0],
            [10.0, 0.0, 10.0],
        );
        let centre = [50.0, 50.0];
        met(
            under(&side, [100, 100], centre, &ALL),
            Xz,
            50.0 * 2f64.sqrt(),
        );
        // XZ not drawn: YZ, behind where it would be.
        met(
            under(&side, [100, 100], centre, &[Xy, Yz]),
            Yz,
            60.0 * 2f64.sqrt(),
        );
        // Neither: nothing.
        assert_eq!(under(&side, [100, 100], centre, &[Xy]), None);
        // Orthographic from above onto (21, 21, 0), 100 mm high on 100 pixels: a millimetre
        // a pixel, x right and y up. The centre is XY's (21, 21, 0), 0 along the ray from the
        // view's plane through the target; XZ and YZ are seen edge on, met nowhere.
        let above = camera(
            Projection::Orthographic { view_height: 100.0 },
            [21.0, 21.0, 100.0],
            [21.0, 21.0, 0.0],
        );
        met(under(&above, [100, 100], centre, &ALL), Xy, 0.0);
        // 24.5 mm right (x 45.5), inside the square; 25.5 (x 46.5), past its side at 46.
        met(under(&above, [100, 100], [74.5, 50.0], &ALL), Xy, 0.0);
        assert_eq!(under(&above, [100, 100], [75.5, 50.0], &ALL), None);
        // Orthographic draws the whole line, in front of the eye too: onto (21, 21, −50)
        // from below the XY plane's back, it's met 50 behind the view's plane.
        let under_it = camera(
            Projection::Orthographic { view_height: 100.0 },
            [21.0, 21.0, -10.0],
            [21.0, 21.0, -50.0],
        );
        met(under(&under_it, [100, 100], centre, &ALL), Xy, -50.0);
        // In perspective, nothing behind the eye: from (20, 20, 10) looking up +z, XY at
        // z = 0 is behind it, and isn't met.
        let up = camera(
            Projection::Perspective {
                field_of_view: 45.0,
            },
            [20.0, 20.0, 10.0],
            [20.0, 20.0, 20.0],
        );
        assert_eq!(under(&up, [100, 100], centre, &ALL), None);
        // A camera `ray` refuses: none.
        let refused = camera(
            Projection::Perspective {
                field_of_view: 45.0,
            },
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        );
        assert_eq!(under(&refused, [100, 100], centre, &ALL), None);
    }

    #[test]
    fn in_perspective_a_plane_met_exactly_the_least_distance_from_the_eye_is_met() {
        // #565's mutants: the drawing cuts what's nearer the eye along the view than
        // MIN_EYE_DISTANCE (1e-6 mm), and draws what's that far: so a plane met exactly that
        // far is met. From (20, 20, 1e-6) looking down −z onto (20, 20, −10), the image's
        // centre looks straight down and meets XY at (20, 20, 0), 1e-6 along the view and
        // along the ray; from (20, 20, 5e-7), half that, it isn't met. By hand (every value
        // here is exact in f64: 1e-6 · 2500 / 2500 is 1e-6, and 1e-6 − 1e-6 is 0).
        let down_from = |height: f64| {
            camera(
                Projection::Perspective {
                    field_of_view: 45.0,
                },
                [20.0, 20.0, height],
                [20.0, 20.0, -10.0],
            )
        };
        let centre = [50.0, 50.0];
        met(
            under(&down_from(MIN_EYE_DISTANCE), [100, 100], centre, &ALL),
            OriginPlane::Xy,
            1e-6,
        );
        assert_eq!(
            under(&down_from(MIN_EYE_DISTANCE / 2.0), [100, 100], centre, &ALL),
            None
        );
    }

    #[test]
    fn two_planes_met_at_the_same_place_go_by_the_browsers_order() {
        // #565's mutants: orthographic from (10, −10, 10) onto (0, 0, 10), the image's centre
        // is the ray from (0, 0, 10) along (−1, 1, 0)/√2, through the Z axis where XZ and YZ
        // cross: both are met there, 0 along it (XY, seen edge on, isn't). XZ comes before
        // YZ in ORIGIN_PLANES, the browser's order, so XZ is the one met; either drawn alone
        // is met there. By hand.
        use OriginPlane::{Xz, Yz};
        let across = camera(
            Projection::Orthographic { view_height: 100.0 },
            [10.0, -10.0, 10.0],
            [0.0, 0.0, 10.0],
        );
        let centre = [50.0, 50.0];
        met(under(&across, [100, 100], centre, &ALL), Xz, 0.0);
        met(under(&across, [100, 100], centre, &[Yz, Xz]), Xz, 0.0);
        met(under(&across, [100, 100], centre, &[Yz]), Yz, 0.0);
        met(under(&across, [100, 100], centre, &[Xz]), Xz, 0.0);
    }

    #[test]
    fn rows_and_planes_name_each_other() {
        for (plane, row) in ORIGIN_PLANES {
            assert_eq!(of_row(row), Some(plane));
            assert_eq!(row_of(plane), row);
        }
        for other in ["Origin", "X", "Y", "Z", "xy", ""] {
            assert_eq!(of_row(other), None, "{other}");
        }
    }
}
