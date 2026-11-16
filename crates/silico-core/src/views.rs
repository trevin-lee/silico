//! The named views a render may ask for, and the camera each makes of the bodies in view
//! (docs/design/m4.6-render.md, *Named views*): Fusion's ViewCube's six faces, eight
//! corners, and twelve edges, in the document's Z-up axes (FreeCAD's), every one
//! orthographic and fitted to the bodies' tight box. Computed in f64, in the order the
//! design writes it, with no trigonometry, so a view's camera is the same on every
//! platform: it's a result.

use silico_protocol::vec3::{cross, dot, sub};
use silico_protocol::{Camera, Class, Projection, ViewRegion};

/// A named view: where it looks from, as Fusion's ViewCube names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NamedView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    TopFrontLeft,
    TopFrontRight,
    TopBackLeft,
    TopBackRight,
    BottomFrontLeft,
    BottomFrontRight,
    BottomBackLeft,
    BottomBackRight,
    TopFront,
    TopBack,
    TopLeft,
    TopRight,
    BottomFront,
    BottomBack,
    BottomLeft,
    BottomRight,
    FrontLeft,
    FrontRight,
    BackLeft,
    BackRight,
}

/// Every named view, in the order above.
pub const NAMED_VIEWS: [NamedView; 26] = [
    NamedView::Front,
    NamedView::Back,
    NamedView::Left,
    NamedView::Right,
    NamedView::Top,
    NamedView::Bottom,
    NamedView::TopFrontLeft,
    NamedView::TopFrontRight,
    NamedView::TopBackLeft,
    NamedView::TopBackRight,
    NamedView::BottomFrontLeft,
    NamedView::BottomFrontRight,
    NamedView::BottomBackLeft,
    NamedView::BottomBackRight,
    NamedView::TopFront,
    NamedView::TopBack,
    NamedView::TopLeft,
    NamedView::TopRight,
    NamedView::BottomFront,
    NamedView::BottomBack,
    NamedView::BottomLeft,
    NamedView::BottomRight,
    NamedView::FrontLeft,
    NamedView::FrontRight,
    NamedView::BackLeft,
    NamedView::BackRight,
];

impl NamedView {
    /// Its name, as a request spells it: `front`, `top_front_right`, `top_front`, ...: a
    /// corner's or an edge's its faces', top or bottom first, then front or back, then left
    /// or right.
    pub fn name(self) -> &'static str {
        match self {
            NamedView::Front => "front",
            NamedView::Back => "back",
            NamedView::Left => "left",
            NamedView::Right => "right",
            NamedView::Top => "top",
            NamedView::Bottom => "bottom",
            NamedView::TopFrontLeft => "top_front_left",
            NamedView::TopFrontRight => "top_front_right",
            NamedView::TopBackLeft => "top_back_left",
            NamedView::TopBackRight => "top_back_right",
            NamedView::BottomFrontLeft => "bottom_front_left",
            NamedView::BottomFrontRight => "bottom_front_right",
            NamedView::BottomBackLeft => "bottom_back_left",
            NamedView::BottomBackRight => "bottom_back_right",
            NamedView::TopFront => "top_front",
            NamedView::TopBack => "top_back",
            NamedView::TopLeft => "top_left",
            NamedView::TopRight => "top_right",
            NamedView::BottomFront => "bottom_front",
            NamedView::BottomBack => "bottom_back",
            NamedView::BottomLeft => "bottom_left",
            NamedView::BottomRight => "bottom_right",
            NamedView::FrontLeft => "front_left",
            NamedView::FrontRight => "front_right",
            NamedView::BackLeft => "back_left",
            NamedView::BackRight => "back_right",
        }
    }

    /// The view named `name`, if there's one.
    pub fn named(name: &str) -> Option<NamedView> {
        NAMED_VIEWS.into_iter().find(|v| v.name() == name)
    }

    /// The sum of the axes of the ViewCube's faces it looks from between (`right` is +x,
    /// `back` +y, `top` +z): a face's axis, an edge's two, a corner's three, each part −1, 0,
    /// or 1. On the cube of side 2 about the origin, it's the face's centre, the edge's
    /// middle, or the corner.
    pub fn axes(self) -> [f64; 3] {
        match self {
            NamedView::Front => [0.0, -1.0, 0.0],
            NamedView::Back => [0.0, 1.0, 0.0],
            NamedView::Left => [-1.0, 0.0, 0.0],
            NamedView::Right => [1.0, 0.0, 0.0],
            NamedView::Top => [0.0, 0.0, 1.0],
            NamedView::Bottom => [0.0, 0.0, -1.0],
            NamedView::TopFrontLeft => [-1.0, -1.0, 1.0],
            NamedView::TopFrontRight => [1.0, -1.0, 1.0],
            NamedView::TopBackLeft => [-1.0, 1.0, 1.0],
            NamedView::TopBackRight => [1.0, 1.0, 1.0],
            NamedView::BottomFrontLeft => [-1.0, -1.0, -1.0],
            NamedView::BottomFrontRight => [1.0, -1.0, -1.0],
            NamedView::BottomBackLeft => [-1.0, 1.0, -1.0],
            NamedView::BottomBackRight => [1.0, 1.0, -1.0],
            NamedView::TopFront => [0.0, -1.0, 1.0],
            NamedView::TopBack => [0.0, 1.0, 1.0],
            NamedView::TopLeft => [-1.0, 0.0, 1.0],
            NamedView::TopRight => [1.0, 0.0, 1.0],
            NamedView::BottomFront => [0.0, -1.0, -1.0],
            NamedView::BottomBack => [0.0, 1.0, -1.0],
            NamedView::BottomLeft => [-1.0, 0.0, -1.0],
            NamedView::BottomRight => [1.0, 0.0, -1.0],
            NamedView::FrontLeft => [-1.0, -1.0, 0.0],
            NamedView::FrontRight => [1.0, -1.0, 0.0],
            NamedView::BackLeft => [-1.0, 1.0, 0.0],
            NamedView::BackRight => [1.0, 1.0, 0.0],
        }
    }

    /// How many of the ViewCube's faces it looks from between: 1 for a face, 2 for an
    /// edge, 3 for a corner (its [`axes`](Self::axes) that aren't 0).
    pub fn faces(self) -> usize {
        self.axes().iter().filter(|part| **part != 0.0).count()
    }

    /// The direction from the target to the eye, a unit vector: its [`axes`](Self::axes)
    /// made a unit, a face's as they are, an edge's times 1/√2 (`FRAC_1_SQRT_2`, the
    /// correctly rounded constant), a corner's times 1/√3 (`1.0 / 3.0f64.sqrt()`).
    pub fn direction(self) -> [f64; 3] {
        let axes = self.axes();
        let unit = match self.faces() {
            1 => 1.0,
            2 => std::f64::consts::FRAC_1_SQRT_2,
            _ => 1.0 / 3.0f64.sqrt(),
        };
        axes.map(|part| part * unit)
    }

    /// Which way is up in the image, a unit vector across the view: [`up_across`] its
    /// direction. So +z for the side faces and the edges between them; +y for `top` and
    /// `bottom` (so `top` shows the front at the image's bottom, as Fusion's and FreeCAD's
    /// do); and for a corner, or an edge of the top or the bottom, +z made perpendicular to
    /// its direction.
    pub fn up(self) -> [f64; 3] {
        up_across(self.direction())
    }

    /// The camera that fits a tight box, `[low, high]`, into an image `width` by `height`:
    /// orthographic, looking at the box's centre from `direction`, its eye a diagonal of
    /// the box away (outside every body, so the near plane cuts nothing), and its view
    /// height 1.1 times the larger of the box's corners' extent along `up` and their extent
    /// along the image's right (`up × direction`) times `height / width`. So the bodies fill
    /// the image with a twentieth of it to spare on each side of the tighter direction.
    pub fn camera(self, bounds: [[f64; 3]; 2], width: u32, height: u32) -> Camera {
        self.fit(bounds, width, height).0
    }

    /// [`camera`](Self::camera), and which of the image's sides bound the fit, `"height"`
    /// or `"width"`: the side whose pixels a view's pixel is, so fewer of them make it
    /// coarser (the extent along `up` over `height`, or along the right over `width`; the
    /// height where they're equal). The view's [`fit_along`] its direction and up.
    pub fn fit(self, bounds: [[f64; 3]; 2], width: u32, height: u32) -> (Camera, &'static str) {
        fit_along(self.direction(), self.up(), bounds, width, height)
    }
}

/// Which way is up looking from `toward`, a unit vector from the target to the eye, as every
/// named view has it ([`NamedView::up`]) and the app's Look At at a face: +z made
/// perpendicular to it (z − (z·toward) toward, made a unit); or +y when its part across the
/// view is under a millionth, too short to steer by, as for `top` and `bottom`.
pub fn up_across(toward: [f64; 3]) -> [f64; 3] {
    let z = [0.0, 0.0, 1.0];
    let along = dot(z, toward);
    let across = [
        z[0] - along * toward[0],
        z[1] - along * toward[1],
        z[2] - along * toward[2],
    ];
    let length = dot(across, across).sqrt();
    if length < 1e-6 {
        return [0.0, 1.0, 0.0];
    }
    across.map(|part| part / length)
}

/// The orthographic camera that fits a tight box, `[low, high]`, into an image `width` by
/// `height`, looking along `direction` (a unit vector from the target to the eye) with `up`
/// (a unit vector across it), and which side bound the fit, as [`NamedView::fit`] gives
/// them for its own direction and up: the box's centre its target, its eye two half
/// diagonals back, and its view height 1.1 times the larger of the box's corners' extent
/// along `up` and their extent along the image's right (`up × direction`) times `height /
/// width`, never under [`least_view_height`] of its half diagonal at `height`. Any client's fit (the
/// app's, its view's direction kept) is this one.
pub fn fit_along(
    direction: [f64; 3],
    up: [f64; 3],
    [low, high]: [[f64; 3]; 2],
    width: u32,
    height: u32,
) -> (Camera, &'static str) {
    let dir = direction;
    let right = cross(up, dir);
    let target = [
        (low[0] + high[0]) / 2.0,
        (low[1] + high[1]) / 2.0,
        (low[2] + high[2]) / 2.0,
    ];
    let mut along_up = (f64::INFINITY, f64::NEG_INFINITY);
    let mut along_right = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let corner = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let (u, r) = (dot(corner, up), dot(corner, right));
        along_up = (along_up.0.min(u), along_up.1.max(u));
        along_right = (along_right.0.min(r), along_right.1.max(r));
    }
    let extent_up = along_up.1 - along_up.0;
    let extent_right = (along_right.1 - along_right.0) * f64::from(height) / f64::from(width);
    let half_diagonal = {
        let d = sub(high, low);
        dot(d, d).sqrt() / 2.0
    };
    // Never under what the render draws at this height of the box's reach from its
    // centre (at most its half diagonal: a rod seen end on), `height × reach / 2^19`,
    // with a hundredth to spare.
    let least = least_view_height(height, half_diagonal);
    let view_height = (1.1 * extent_up.max(extent_right)).max(least);
    let side = if extent_up >= extent_right {
        "height"
    } else {
        "width"
    };
    let eye = [
        target[0] + dir[0] * 2.0 * half_diagonal,
        target[1] + dir[1] * 2.0 * half_diagonal,
        target[2] + dir[2] * 2.0 * half_diagonal,
    ];
    (
        Camera {
            projection: Projection::Orthographic { view_height },
            eye,
            target,
            up,
        },
        side,
    )
}

/// The least height an orthographic view `height` pixels high draws bodies reaching `reach`
/// mm from its target at: a renderer drawing in f32 refuses a view under `height × reach /
/// 2^19` (`silico_protocol::least_view_height`), and this leaves a hundredth to spare.
pub fn least_view_height(height: u32, reach: f64) -> f64 {
    silico_protocol::least_view_height(height, reach) * 1.01
}

/// How far from `from` the farthest corner of any of `boxes` is: the bodies' reach, as a
/// renderer measures it from an orthographic view's target. 0 for no box.
pub fn reach(from: [f64; 3], boxes: impl IntoIterator<Item = [[f64; 3]; 2]>) -> f64 {
    boxes.into_iter().fold(0.0, |far, [low, high]| {
        let d = [0, 1, 2].map(|k| (low[k] - from[k]).abs().max((high[k] - from[k]).abs()));
        far.max(dot(d, d).sqrt())
    })
}

/// The smallest box holding every one of `boxes`, if there are any.
pub fn joined(boxes: impl IntoIterator<Item = [[f64; 3]; 2]>) -> Option<[[f64; 3]; 2]> {
    boxes.into_iter().reduce(|[low, high], [l, h]| {
        [
            [low[0].min(l[0]), low[1].min(l[1]), low[2].min(l[2])],
            [high[0].max(h[0]), high[1].max(h[1]), high[2].max(h[2])],
        ]
    })
}

/// Half a pixel of `camera`'s image, `height` pixels high, in mm, at `depth` along the view
/// from a perspective eye (an orthographic view's is the same at any): a body is meshed to
/// half a pixel at its nearest depth (`depth_of_box`), where its pixels are finest. None for
/// a camera that fails its check.
pub fn half_pixel_at(camera: &Camera, height: u32, depth: f64) -> Option<f64> {
    camera.frame().ok()?;
    let view_height = match camera.projection {
        Projection::Orthographic { view_height } => view_height,
        Projection::Perspective { field_of_view } => {
            2.0 * depth * libm::tan(field_of_view.to_radians() / 2.0)
        }
    };
    Some(view_height / f64::from(height) / 2.0)
}

/// How deep along `camera`'s view its nearest point is: the least of its eight corners'
/// depths from the eye along the view's direction, never under 0 (a box reaching the eye's
/// plane). A point's pixel scales with its depth, so a body is meshed to half a pixel there.
pub fn depth_of_box(camera: &Camera, [low, high]: [[f64; 3]; 2]) -> f64 {
    let Ok(frame) = camera.frame() else {
        return 0.0;
    };
    let mut least = f64::INFINITY;
    for i in 0..8 {
        let corner = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let d = [
            corner[0] - camera.eye[0],
            corner[1] - camera.eye[1],
            corner[2] - camera.eye[2],
        ];
        least = least.min(-dot(d, frame.back));
    }
    least.max(0.0)
}

/// The depth a body is meshed for (docs/design/m5.0-app.md, `meshes`): its box's nearest
/// depth along the view (`depth_of_box`), or, for a box wholly behind the eye's plane, the
/// distance from the eye to the box's nearest point. So a body just behind the camera has
/// about the level it needs when a turn brings it into view, and one far behind a coarse
/// one, where the depth along the view would give every such body the finest mesh there is
/// (it's 0 for them). `render` culls those bodies, so it meets only `depth_of_box`'s case.
pub fn meshing_depth(camera: &Camera, [low, high]: [[f64; 3]; 2]) -> f64 {
    let Ok(frame) = camera.frame() else {
        return 0.0;
    };
    let mut farthest = f64::NEG_INFINITY;
    for i in 0..8 {
        let corner = [
            if i & 1 == 0 { low[0] } else { high[0] },
            if i & 2 == 0 { low[1] } else { high[1] },
            if i & 4 == 0 { low[2] } else { high[2] },
        ];
        let d = [
            corner[0] - camera.eye[0],
            corner[1] - camera.eye[1],
            corner[2] - camera.eye[2],
        ];
        farthest = farthest.max(-dot(d, frame.back));
    }
    if farthest >= 0.0 {
        return depth_of_box(camera, [low, high]);
    }
    // Wholly behind: the eye's distance to the box, from its nearest point.
    let gap = |k: usize| {
        (low[k] - camera.eye[k])
            .max(0.0)
            .max(camera.eye[k] - high[k])
    };
    let (x, y, z) = (gap(0), gap(1), gap(2));
    libm::sqrt(x * x + y * y + z * z)
}

/// Whether `camera`'s view, `width` by `height` pixels, may see any of a tight box,
/// `[low, high]`: false only when the whole box is outside the view (beside an orthographic
/// view's sides, which don't clip along the view; or behind a perspective eye, or past one
/// of its sides), so a body left out is never one the image shows. None for a camera that
/// fails its check.
pub fn sees(camera: &Camera, width: u32, height: u32, [low, high]: [[f64; 3]; 2]) -> Option<bool> {
    let frame = camera.frame().ok()?;
    // The image's frustum, as the renderer culls bodies and faces by it.
    Some(ViewRegion::frustum(camera, &frame, width, height).class_of_box((low, high)) != Class::Out)
}

#[cfg(test)]
mod tests {
    use super::{
        NAMED_VIEWS, NamedView, cross, depth_of_box, dot, fit_along, half_pixel_at, joined,
        least_view_height, meshing_depth, reach, sees, up_across,
    };
    use silico_protocol::{Camera, Projection};

    const BLOCK: [[f64; 3]; 2] = [[0.0, 0.0, 0.0], [40.0, 20.0, 10.0]];

    /// A camera at E = (10, 20, 30) looking along `forward` (unnormalized), `up` up, and a
    /// box `[low, high]` given from E.
    fn from_e(
        forward: [f64; 3],
        up: [f64; 3],
        low: [f64; 3],
        high: [f64; 3],
    ) -> (Camera, [[f64; 3]; 2]) {
        let e = [10.0, 20.0, 30.0];
        let camera = Camera {
            projection: Projection::Orthographic { view_height: 100.0 },
            eye: e,
            target: [e[0] + forward[0], e[1] + forward[1], e[2] + forward[2]],
            up,
        };
        let at = |v: [f64; 3]| [e[0] + v[0], e[1] + v[1], e[2] + v[2]];
        (camera, [at(low), at(high)])
    }

    #[test]
    fn a_box_with_one_corner_ahead_of_the_eye_is_meshed_at_depth_0_whichever_its_axis() {
        // Each view along a diagonal of two axes, so one corner alone (the one low or high
        // on each) is ahead of the eye's plane, by 1/√2, and every other behind: the box
        // reaches the eye's plane, so its depth is 0 (`depth_of_box`'s least, -8/√2, held
        // at 0). Were that corner left out, the box would be wholly behind, and its depth the
        // eye's distance to it, 1. Along (−1, 1, 0): the low-x, high-y corner (1, 2), others
        // (5, 2), (1, −3), (5, −3) behind. Along (1, −1, 0): high x, low y. Along (1, 0, −1):
        // high x, low z.
        let cases = [
            from_e(
                [-1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, -3.0, -1.0],
                [5.0, 2.0, 1.0],
            ),
            from_e(
                [1.0, -1.0, 0.0],
                [0.0, 0.0, 1.0],
                [-5.0, -2.0, -1.0],
                [-1.0, 3.0, 1.0],
            ),
            from_e(
                [1.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
                [-5.0, -1.0, -2.0],
                [-1.0, 1.0, 3.0],
            ),
        ];
        for (n, (camera, bounds)) in cases.iter().enumerate() {
            assert_eq!(meshing_depth(camera, *bounds), 0.0, "case {n}");
        }
    }

    #[test]
    fn a_box_wholly_behind_the_eye_is_meshed_at_its_distance_from_the_eye() {
        // Looking along −z from E, a box behind it, at x from 1 to 2, y from −6 to −3, z from
        // 4 to 6 (from E): its nearest point is 1, 3, and 4 away along the axes, so √26 (no two
        // alike, and none 2, where a square is a double).
        let (camera, bounds) = from_e(
            [0.0, 0.0, -1.0],
            [0.0, 1.0, 0.0],
            [1.0, -6.0, 4.0],
            [2.0, -3.0, 6.0],
        );
        assert_eq!(meshing_depth(&camera, bounds), libm::sqrt(26.0));
    }

    #[test]
    fn the_side_that_bounds_a_fit_is_the_one_whose_extent_over_its_pixels_is_larger() {
        // The block, top_front_right at 800 x 600: 32.660 along up, against 42.426 × 0.75 =
        // 31.820 along the right: the height. From the front: 10 along up (z), against 40 ×
        // 0.75 = 30 along the right (x): the width. A cube from the front, square: equal, the
        // height.
        assert_eq!(NamedView::TopFrontRight.fit(BLOCK, 800, 600).1, "height");
        assert_eq!(NamedView::Front.fit(BLOCK, 800, 600).1, "width");
        // 40 wide at 3200 pixels: 30 × 600/3200 is under 10, so the height.
        assert_eq!(NamedView::Front.fit(BLOCK, 3200, 600).1, "height");
        let cube = [[0.0; 3], [10.0; 3]];
        assert_eq!(NamedView::Front.fit(cube, 300, 300).1, "height");
        assert_eq!(NamedView::Front.fit(cube, 300, 301).1, "width");
        assert_eq!(
            NamedView::Front.fit(BLOCK, 800, 600).0,
            NamedView::Front.camera(BLOCK, 800, 600)
        );
    }

    #[test]
    fn the_designs_example_is_the_blocks_top_front_right_at_800_by_600() {
        // docs/design/m4.6-render.md, Named views: the 40 x 20 x 10 block, top_front_right,
        // 800 x 600. By hand: its up is (−1, 1, 2)/√6; its extents along up and right are
        // 32.660 and 42.426, so its view height is 1.1 × max(32.660, 42.426 × 0.75); its eye
        // is the box's diagonal, √2100, from its centre.
        let camera = NamedView::TopFrontRight.camera(BLOCK, 800, 600);
        assert_eq!(camera.target, [20.0, 10.0, 5.0]);
        let s6 = 6f64.sqrt();
        let want_up = [-1.0 / s6, 1.0 / s6, 2.0 / s6];
        for (got, want) in camera.up.iter().zip(want_up) {
            assert!((got - want).abs() <= 1e-15, "{:?}", camera.up);
        }
        assert_eq!(
            camera.up,
            [
                -0.408_248_290_463_863_13,
                0.408_248_290_463_863_13,
                0.816_496_580_927_725_8
            ]
        );
        let Projection::Orthographic { view_height } = camera.projection else {
            panic!("{:?}", camera.projection);
        };
        assert_eq!(view_height, 35.925_849_560_819_955);
        let d = [
            camera.eye[0] - 20.0,
            camera.eye[1] - 10.0,
            camera.eye[2] - 5.0,
        ];
        assert!((dot(d, d).sqrt() - 2100f64.sqrt()).abs() <= 1e-12);
    }

    #[test]
    fn every_view_is_orthonormal_not_mirrored_and_named_as_its_direction_says() {
        for view in NAMED_VIEWS {
            let (dir, up) = (view.direction(), view.up());
            assert!((dot(dir, dir) - 1.0).abs() <= 1e-15, "{view:?}");
            assert!((dot(up, up) - 1.0).abs() <= 1e-15, "{view:?}");
            assert!(dot(dir, up).abs() <= 1e-15, "{view:?}");
            // The image's right, up × dir: from the front, +x; from the right, +y (the back
            // to the right); from the top, +x with +y up.
            let right = cross(up, dir);
            match view {
                NamedView::Front => assert_eq!(
                    (dir, up, right),
                    ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0])
                ),
                NamedView::Right => assert_eq!(
                    (dir, up, right),
                    ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
                ),
                NamedView::Back => assert_eq!(right, [-1.0, 0.0, 0.0]),
                NamedView::Left => assert_eq!(right, [0.0, -1.0, 0.0]),
                NamedView::Top => assert_eq!(
                    (dir, up, right),
                    ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0])
                ),
                NamedView::Bottom => assert_eq!((up, right), ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0])),
                _ => {
                    // A corner's or an edge's name says its direction's signs, 0 along an
                    // axis it names neither end of; its up has +z.
                    let name = view.name();
                    let sign = |yes: &str, no: &str| {
                        if name.contains(yes) {
                            assert!(!name.contains(no));
                            1.0
                        } else if name.contains(no) {
                            -1.0
                        } else {
                            0.0
                        }
                    };
                    let want = [
                        sign("right", "left"),
                        sign("back", "front"),
                        sign("top", "bottom"),
                    ];
                    for k in 0..3 {
                        let got = if dir[k] == 0.0 { 0.0 } else { dir[k].signum() };
                        assert_eq!(got, want[k], "{view:?}");
                    }
                    // Two faces' or three: an edge or a corner, never a face again.
                    assert!(want.iter().filter(|w| **w != 0.0).count() >= 2, "{view:?}");
                    assert!(up[2] > 0.0, "{view:?}");
                }
            }
            assert_eq!(NamedView::named(view.name()), Some(view));
        }
        assert_eq!(NamedView::named("front_top"), None);
        assert_eq!(NamedView::named("Front"), None);
        assert_eq!(NamedView::named("left_front"), None);
        assert_eq!(NamedView::named("top_bottom"), None);
        assert_eq!(NamedView::named("top_front_"), None);
        // Each named once: 26 names, none twice.
        let names: std::collections::BTreeSet<&str> =
            NAMED_VIEWS.iter().map(|view| view.name()).collect();
        assert_eq!(names.len(), 26);
    }

    #[test]
    fn each_edges_view_is_between_its_two_faces_with_z_made_perpendicular_up() {
        // By hand: an edge's direction is its two faces' axes over √2. Its up is
        // z − (z·d) d made a unit: for d = (a, b, c)/√2, z·d = c/√2, so z − (c/2)(a, b, c)
        // = (−ca/2, −cb/2, 1 − c²/2): for a horizontal edge (c = 0), z itself; for a top
        // or bottom edge (c = ±1, one of a and b 0), (−ca, −cb, 1)/2, whose length is
        // 1/√2, so (−ca, −cb, 1)/√2. Each written here as numerators over √2.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let z = [0.0, 0.0, 1.0];
        for (name, direction, up) in [
            ("top_front", [0.0, -1.0, 1.0], Some([0.0, 1.0, 1.0])),
            ("top_back", [0.0, 1.0, 1.0], Some([0.0, -1.0, 1.0])),
            ("top_left", [-1.0, 0.0, 1.0], Some([1.0, 0.0, 1.0])),
            ("top_right", [1.0, 0.0, 1.0], Some([-1.0, 0.0, 1.0])),
            ("bottom_front", [0.0, -1.0, -1.0], Some([0.0, -1.0, 1.0])),
            ("bottom_back", [0.0, 1.0, -1.0], Some([0.0, 1.0, 1.0])),
            ("bottom_left", [-1.0, 0.0, -1.0], Some([-1.0, 0.0, 1.0])),
            ("bottom_right", [1.0, 0.0, -1.0], Some([1.0, 0.0, 1.0])),
            ("front_left", [-1.0, -1.0, 0.0], None),
            ("front_right", [1.0, -1.0, 0.0], None),
            ("back_left", [-1.0, 1.0, 0.0], None),
            ("back_right", [1.0, 1.0, 0.0], None),
        ] {
            let view = NamedView::named(name).unwrap();
            assert_eq!(view.name(), name);
            assert_eq!(view.axes(), direction, "{name}");
            // The direction exactly the axes times 1/√2's double: ±1 · s is exact.
            assert_eq!(view.direction(), direction.map(|k| k * s), "{name}");
            match up {
                // A horizontal edge's up is z exactly: z·d is 0.
                None => assert_eq!(view.up(), z, "{name}"),
                Some(up) => {
                    for (got, want) in view.up().iter().zip(up.map(|k| k * s)) {
                        assert!((got - want).abs() <= 1e-15, "{name}: {:?}", view.up());
                    }
                }
            }
        }
        // And to the bit, as #471's ViewCube gave them (`t` its middle times `FRAC_1_SQRT_2`,
        // up z − (z·t) t over its length), worked apart from the code in IEEE doubles: the
        // parts of a top or bottom edge's up are not √2/2's double, 0.7071067811865476, but
        // 0.7071067811865477 across and 0.7071067811865474 up, as `1 − s²` and `s²` round.
        let (a, b) = (0.707_106_781_186_547_7, 0.707_106_781_186_547_4);
        for (view, up) in [
            (NamedView::TopFront, [0.0, a, b]),
            (NamedView::TopBack, [0.0, -a, b]),
            (NamedView::TopLeft, [a, 0.0, b]),
            (NamedView::TopRight, [-a, 0.0, b]),
            (NamedView::BottomFront, [0.0, -a, b]),
            (NamedView::BottomBack, [0.0, a, b]),
            (NamedView::BottomLeft, [-a, 0.0, b]),
            (NamedView::BottomRight, [a, 0.0, b]),
            (NamedView::FrontLeft, z),
            (NamedView::FrontRight, z),
            (NamedView::BackLeft, z),
            (NamedView::BackRight, z),
        ] {
            assert_eq!(
                view.up().map(f64::to_bits),
                up.map(f64::to_bits),
                "{view:?}"
            );
        }
        // The rule for every view, z made perpendicular to the direction or +y for the top
        // and the bottom, is `up_across`'s.
        for view in NAMED_VIEWS {
            assert_eq!(view.up(), up_across(view.direction()), "{view:?}");
        }
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
        for (got, want) in up_across(corner).iter().zip([-1.0 / s, -1.0 / s, 2.0 / s]) {
            assert!((got - want).abs() <= 1e-15, "{got} against {want}");
        }
        // Within a millionth of z, still z's; a face a hundredth off it turns z with it.
        assert_eq!(up_across([0.0, 1e-7, 1.0]), [0.0, 1.0, 0.0]);
        // At exactly a millionth (from (0, 1e-6, 1), +z's part across is (0, −1e-6, 0), its
        // length 1e-6 to the bit, as √(x²) is x), it steers by that part: −y, not +y.
        assert_eq!(up_across([0.0, 1e-6, 1.0]), [0.0, -1.0, 0.0]);
        let tipped = [0.0, -0.01, (1.0f64 - 1e-4).sqrt()];
        let up = up_across(tipped);
        assert!(up[1] > 0.99 && up[2] > 0.0, "{up:?}");
    }

    #[test]
    fn the_blocks_top_front_view_at_800_by_600_is_its_camera_to_the_bit() {
        // As the design's example pins top_front_right's: the block, top_front, 800 x 600.
        // By hand, the width binds, 1.1 × 40 × 0.75 = 33 (exactly 33 in doubles); the eye is
        // the box's diagonal, √2100, from its centre (20, 10, 5) along (0, −1, 1)/√2:
        // (20, 10 − √1050, 5 + √1050). Its digits worked in IEEE doubles apart from the code,
        // as `fit_along` orders them: the centre plus the direction's part times twice the
        // half diagonal.
        let (camera, side) = NamedView::TopFront.fit(BLOCK, 800, 600);
        assert_eq!(side, "width");
        assert_eq!(camera.target, [20.0, 10.0, 5.0]);
        assert_eq!(
            camera.eye.map(f64::to_bits),
            [20.0, -22.403_703_492_039_3, 37.403_703_492_039_3].map(f64::to_bits)
        );
        let root = 1050f64.sqrt();
        assert!((camera.eye[1] - (10.0 - root)).abs() <= 1e-13);
        assert!((camera.eye[2] - (5.0 + root)).abs() <= 1e-13);
        assert_eq!(
            camera.projection,
            Projection::Orthographic { view_height: 33.0 }
        );
        assert_eq!(
            camera.up.map(f64::to_bits),
            [0.0, 0.707_106_781_186_547_7, 0.707_106_781_186_547_4].map(f64::to_bits)
        );
    }

    #[test]
    fn the_blocks_edge_views_fit_it_as_worked_by_hand() {
        // The 40 x 20 x 10 block about (20, 10, 5), at 800 x 600, so the right's extent
        // counts 0.75 of itself against the up's. Its eye √2100, the box's diagonal, from
        // its centre along the direction.
        // top_front: up (0, 1, 1)/√2, right up × dir = (1, 0, 0). Along up the corners
        // (y + z)/√2 span 30/√2 = 21.213; along the right x spans 40, × 0.75 = 30: the
        // width binds, 1.1 × 30 = 33.
        // top_left: up (1, 0, 1)/√2, right (0, −1, 0). Along up (x + z)/√2 spans 50/√2 =
        // 35.355; along the right y spans 20, × 0.75 = 15: the height, 1.1 × 50/√2.
        // front_right: up z, right (1, 1, 0)/√2. Along up z spans 10; along the right
        // (x + y)/√2 spans 60/√2 = 42.426, × 0.75 = 31.820: the width, 1.1 × 0.75 × 60/√2.
        let r2 = 2f64.sqrt();
        for (view, right, height, side) in [
            (NamedView::TopFront, [1.0, 0.0, 0.0], 33.0, "width"),
            (
                NamedView::TopLeft,
                [0.0, -1.0, 0.0],
                1.1 * 50.0 / r2,
                "height",
            ),
            (
                NamedView::FrontRight,
                [1.0 / r2, 1.0 / r2, 0.0],
                1.1 * 0.75 * 60.0 / r2,
                "width",
            ),
        ] {
            let (camera, bound) = view.fit(BLOCK, 800, 600);
            assert_eq!(bound, side, "{view:?}");
            assert_eq!(camera.target, [20.0, 10.0, 5.0]);
            let Projection::Orthographic { view_height } = camera.projection else {
                panic!("{view:?}: {:?}", camera.projection);
            };
            assert!(
                (view_height - height).abs() <= 1e-12,
                "{view:?}: {view_height}"
            );
            let got_right = cross(camera.up, view.direction());
            for (got, want) in got_right.iter().zip(right) {
                assert!((got - want).abs() <= 1e-15, "{view:?}: {got_right:?}");
            }
            let d = [
                camera.eye[0] - 20.0,
                camera.eye[1] - 10.0,
                camera.eye[2] - 5.0,
            ];
            let away = 2100f64.sqrt();
            for (got, axis) in d.iter().zip(view.axes()) {
                let want = axis / r2 * away;
                assert!(
                    (got - want).abs() <= 1e-12,
                    "{view:?}: eye {:?}",
                    camera.eye
                );
            }
        }
    }

    #[test]
    fn every_view_fits_the_box_with_a_twentieth_to_spare_on_its_tighter_side() {
        for (width, height) in [(800, 600), (300, 900), (1, 1), (4096, 1)] {
            for view in NAMED_VIEWS {
                let camera = view.camera(BLOCK, width, height);
                camera.check(width, height).unwrap();
                let Projection::Orthographic { view_height } = camera.projection else {
                    panic!();
                };
                let view_width = view_height * f64::from(width) / f64::from(height);
                let right = cross(camera.up, view.direction());
                let (mut u_most, mut r_most) = (0.0f64, 0.0f64);
                for i in 0..8 {
                    let c = [
                        if i & 1 == 0 { 0.0 } else { 40.0 },
                        if i & 2 == 0 { 0.0 } else { 20.0 },
                        if i & 4 == 0 { 0.0 } else { 10.0 },
                    ];
                    let d = [c[0] - 20.0, c[1] - 10.0, c[2] - 5.0];
                    u_most = u_most.max(dot(d, camera.up).abs());
                    r_most = r_most.max(dot(d, right).abs());
                }
                // Every corner inside, and the tighter side exactly 1/1.1 of the view.
                let (u_share, r_share) = (2.0 * u_most / view_height, 2.0 * r_most / view_width);
                assert!(
                    u_share <= 1.0 / 1.1 + 1e-12 && r_share <= 1.0 / 1.1 + 1e-12,
                    "{view:?} {width}x{height}"
                );
                assert!(
                    (u_share.max(r_share) - 1.0 / 1.1).abs() <= 1e-12,
                    "{view:?} {width}x{height}"
                );
                // The eye outside the box, along the view's direction.
                let d = [
                    camera.eye[0] - 20.0,
                    camera.eye[1] - 10.0,
                    camera.eye[2] - 5.0,
                ];
                assert!(dot(d, view.direction()) > 2100f64.sqrt() / 2.0, "{view:?}");
            }
        }
    }

    /// From −y, looking at the origin, up +z, 100 away: orthographic 44 high, or
    /// perspective 40° high.
    fn from_front(projection: Projection) -> Camera {
        Camera {
            projection,
            eye: [0.0, -100.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    const ORTHO: Projection = Projection::Orthographic { view_height: 44.0 };
    const PERSPECTIVE: Projection = Projection::Perspective {
        field_of_view: 40.0,
    };

    #[test]
    fn half_a_pixel_is_the_view_over_its_pixels_at_a_depth() {
        // Orthographic, 44 high over 400 pixels, the same at any depth.
        for depth in [0.0, 100.0, 1e6] {
            assert_eq!(
                half_pixel_at(&from_front(ORTHO), 400, depth),
                Some(44.0 / 400.0 / 2.0)
            );
        }
        // Perspective, 40° high: 2 × depth × tan 20° over 200 pixels, halved; tan 20° by
        // hand.
        let tan20 = 0.363_970_234_266_202_4;
        for depth in [1.0, 100.0] {
            let got = half_pixel_at(&from_front(PERSPECTIVE), 200, depth).unwrap();
            assert!((got - depth * tan20 / 200.0).abs() <= 1e-15, "{got}");
        }
        let mut bad = from_front(ORTHO);
        bad.eye = bad.target;
        assert_eq!(half_pixel_at(&bad, 200, 1.0), None);
    }

    #[test]
    fn a_boxs_depth_is_its_nearest_corners_along_the_view_never_under_0() {
        // From −y at y = −100, looking along +y: a box from y = 0 is 100 deep however far
        // aside it lies (its distance would grow, its depth doesn't); one reaching y = −150,
        // behind the eye, is at no depth.
        let camera = from_front(PERSPECTIVE);
        assert_eq!(
            depth_of_box(&camera, [[-1.0, 0.0, -1.0], [1.0, 10.0, 1.0]]),
            100.0
        );
        assert_eq!(
            depth_of_box(&camera, [[300.0, 0.0, 40.0], [310.0, 5.0, 50.0]]),
            100.0
        );
        assert_eq!(
            depth_of_box(&camera, [[-1.0, -150.0, -1.0], [1.0, 0.0, 1.0]]),
            0.0
        );
        assert_eq!(
            depth_of_box(&camera, [[-1.0, -99.5, -1.0], [1.0, 0.0, 1.0]]),
            0.5
        );
    }

    #[test]
    fn a_box_is_left_out_only_when_wholly_outside_the_view() {
        let cube = |c: [f64; 3], half: f64| {
            [
                [c[0] - half, c[1] - half, c[2] - half],
                [c[0] + half, c[1] + half, c[2] + half],
            ]
        };
        // Orthographic, 44 high and wide (square): x and z within ±22 are in view, at any
        // depth (nothing clipped behind the eye).
        let ortho = from_front(ORTHO);
        for (centre, half, seen) in [
            ([0.0, 0.0, 0.0], 1.0, true),
            ([0.0, 500.0, 0.0], 1.0, true),
            ([0.0, -500.0, 0.0], 1.0, true),
            ([22.5, 0.0, 0.0], 1.0, true),
            ([23.5, 0.0, 0.0], 1.0, false),
            ([0.0, 0.0, -23.5], 1.0, false),
            ([40.0, 0.0, 40.0], 30.0, true),
        ] {
            assert_eq!(
                sees(&ortho, 300, 300, cube(centre, half)),
                Some(seen),
                "{centre:?} {half}"
            );
        }
        // Twice as wide as high: x within ±44 in view.
        assert_eq!(
            sees(&ortho, 600, 300, cube([40.0, 0.0, 0.0], 1.0)),
            Some(true)
        );
        // Perspective, 40° high, square: at the target, 100 from the eye, ±36.4 in view;
        // behind the eye, nothing.
        let perspective = from_front(PERSPECTIVE);
        for (centre, half, seen) in [
            ([0.0, 0.0, 0.0], 1.0, true),
            ([35.0, 0.0, 0.0], 1.0, true),
            ([38.0, 0.0, 0.0], 1.0, false),
            ([0.0, 0.0, -38.0], 1.0, false),
            ([0.0, -150.0, 0.0], 1.0, false),
            // Around the eye: corners on every side.
            ([0.0, -100.0, 0.0], 5.0, true),
            ([0.0, 900.0, 0.0], 1.0, true),
        ] {
            assert_eq!(
                sees(&perspective, 300, 300, cube(centre, half)),
                Some(seen),
                "{centre:?} {half}"
            );
        }
    }

    #[test]
    fn a_named_views_depth_is_its_eyes_distance_less_how_far_the_nearest_corner_reaches() {
        // Each view's eye is the block's diagonal, √2100, from its centre along its
        // direction. By hand, the nearest corner reaches toward the eye by half the block
        // along a face's axis (y 10, x 20, z 5), by the two halves' sum over √2 along an
        // edge's (the top's and the front's, (5 + 10)/√2), or by (20 + 10 + 5)/√3 along a
        // corner's diagonal; each view's nearest corner is another of the eight, so every
        // one counts.
        let diagonal = 2100f64.sqrt();
        let r2 = 2f64.sqrt();
        for view in NAMED_VIEWS {
            let reach = match view {
                NamedView::Front | NamedView::Back => 10.0,
                NamedView::Left | NamedView::Right => 20.0,
                NamedView::Top | NamedView::Bottom => 5.0,
                NamedView::TopFront
                | NamedView::TopBack
                | NamedView::BottomFront
                | NamedView::BottomBack => 15.0 / r2,
                NamedView::TopLeft
                | NamedView::TopRight
                | NamedView::BottomLeft
                | NamedView::BottomRight => 25.0 / r2,
                NamedView::FrontLeft
                | NamedView::FrontRight
                | NamedView::BackLeft
                | NamedView::BackRight => 30.0 / r2,
                _ => 35.0 / 3f64.sqrt(),
            };
            let depth = depth_of_box(&view.camera(BLOCK, 800, 600), BLOCK);
            assert!(
                (depth - (diagonal - reach)).abs() <= 1e-12,
                "{view:?}: {depth}, by hand {}",
                diagonal - reach
            );
        }
    }

    #[test]
    fn an_orthographic_box_in_view_by_one_corner_alone_is_seen() {
        // Looking down −z with up (−1, 1, 0)/√2 (so the right is (1, 1, 0)/√2), 44 high,
        // square: within ±22 along each. The box x −18..−8, y 18..28 reaches 26/√2 = 18.4 up
        // at its corner (−8, 18), and 25.5 up at its others: in view by that corner alone,
        // across from 0 to 14.1. From −y with up (−1, 0, 1)/√2, the same box with z for y.
        let cases = [
            (
                [0.0, 0.0, 100.0],
                [-1.0, 1.0, 0.0],
                [[-18.0, 18.0, -1.0], [-8.0, 28.0, 1.0]],
                1,
            ),
            (
                [0.0, -100.0, 0.0],
                [-1.0, 0.0, 1.0],
                [[-18.0, -1.0, 18.0], [-8.0, 1.0, 28.0]],
                2,
            ),
        ];
        for (eye, up, bounds, k) in cases {
            let camera = Camera {
                projection: ORTHO,
                eye,
                target: [0.0; 3],
                up,
            };
            assert_eq!(sees(&camera, 300, 300, bounds), Some(true), "{up:?}");
            // 6 further along y (or z), its nearest corner 32/√2 = 22.6 up: out of view.
            let [mut low, mut high] = bounds;
            low[k] += 6.0;
            high[k] += 6.0;
            assert_eq!(sees(&camera, 300, 300, [low, high]), Some(false), "{up:?}");
        }
    }

    #[test]
    fn an_orthographic_view_sees_about_its_target_wherever_that_is() {
        // From +z onto (30, 50, 0), up +y, 44 high and square: x 8..52 and y 28..72 in view.
        let camera = Camera {
            projection: ORTHO,
            eye: [30.0, 50.0, 100.0],
            target: [30.0, 50.0, 0.0],
            up: [0.0, 1.0, 0.0],
        };
        let cube = |c: [f64; 3]| [[c[0] - 1.0, c[1] - 1.0, c[2] - 1.0], c.map(|v| v + 1.0)];
        for (centre, seen) in [
            ([30.0, 50.0, 0.0], true),
            ([30.0, 50.0, -500.0], true),
            ([0.0, 0.0, 0.0], false),
            ([30.0, 0.0, 0.0], false),
            ([0.0, 50.0, 0.0], false),
            ([30.0, 74.0, 0.0], false),
        ] {
            assert_eq!(
                sees(&camera, 300, 300, cube(centre)),
                Some(seen),
                "{centre:?}"
            );
        }
    }

    #[test]
    fn a_perspective_view_is_out_past_each_of_its_four_sides_and_in_on_them() {
        // From −y at y = −100, 40° high: at y = 0, 100 deep, the sides are at ±100 tan 20°
        // (36.4) across and up when square, and twice that across when twice as wide.
        let perspective = from_front(PERSPECTIVE);
        let cube = |c: [f64; 3]| [[c[0] - 1.0, c[1] - 1.0, c[2] - 1.0], c.map(|v| v + 1.0)];
        for (centre, seen) in [
            ([-35.0, 0.0, 0.0], true),
            ([-38.0, 0.0, 0.0], false),
            ([0.0, 0.0, 35.0], true),
            ([0.0, 0.0, 38.0], false),
        ] {
            assert_eq!(
                sees(&perspective, 300, 300, cube(centre)),
                Some(seen),
                "{centre:?}"
            );
        }
        for (centre, seen) in [
            ([60.0, 0.0, 0.0], true),
            ([-60.0, 0.0, 0.0], true),
            ([0.0, 0.0, 38.0], false),
        ] {
            assert_eq!(
                sees(&perspective, 600, 300, cube(centre)),
                Some(seen),
                "{centre:?} at 600 x 300"
            );
        }
        // An eye off the axes, (50, −100, 30), onto (50, 0, 30): the box there is seen, and
        // one 30 below it (within 36.4); one at the origin, 50 to the left, isn't.
        let aside = Camera {
            eye: [50.0, -100.0, 30.0],
            target: [50.0, 0.0, 30.0],
            ..perspective.clone()
        };
        assert_eq!(sees(&aside, 300, 300, cube([50.0, 0.0, 30.0])), Some(true));
        assert_eq!(sees(&aside, 300, 300, cube([50.0, 0.0, 0.0])), Some(true));
        assert_eq!(sees(&aside, 300, 300, cube([0.0, 0.0, 0.0])), Some(false));
        // A box 100 deep with a face exactly on a side touches the view: seen. Moved off it
        // by the least there is, it's out. The side, 100 × tan 20°, as the view takes it.
        let side = 100.0 * libm::tan(40f64.to_radians() / 2.0);
        let past = side.next_up();
        let flat = |x: [f64; 2], z: [f64; 2]| [[x[0], 0.0, z[0]], [x[1], 0.0, z[1]]];
        for (on, off, which) in [
            (
                flat([side, side + 10.0], [-1.0, 1.0]),
                flat([past, past + 10.0], [-1.0, 1.0]),
                "right",
            ),
            (
                flat([-side - 10.0, -side], [-1.0, 1.0]),
                flat([-past - 10.0, -past], [-1.0, 1.0]),
                "left",
            ),
            (
                flat([-1.0, 1.0], [side, side + 10.0]),
                flat([-1.0, 1.0], [past, past + 10.0]),
                "top",
            ),
            (
                flat([-1.0, 1.0], [-side - 10.0, -side]),
                flat([-1.0, 1.0], [-past - 10.0, -past]),
                "bottom",
            ),
        ] {
            assert_eq!(sees(&perspective, 300, 300, on), Some(true), "{which}");
            assert_eq!(sees(&perspective, 300, 300, off), Some(false), "{which}");
        }
    }

    #[test]
    fn a_fit_along_any_direction_is_the_named_views_fit_along_theirs() {
        // Home's direction on a 20 mm cube, square: up (−1, 1, 2)/√6 spans 80/√6 of the
        // cube (its corners (0, 20, 20) and (20, 0, 0)), so 1.1 · 80/√6 = 35.926 high.
        let cube = [[0.0; 3], [20.0; 3]];
        let (camera, side) = fit_along(
            NamedView::TopFrontRight.direction(),
            NamedView::TopFrontRight.up(),
            cube,
            400,
            400,
        );
        let Projection::Orthographic { view_height } = camera.projection else {
            panic!("orthographic")
        };
        assert!((view_height - 1.1 * 80.0 / 6f64.sqrt()).abs() < 1e-12);
        assert_eq!(side, "height");
        for view in NAMED_VIEWS {
            assert_eq!(
                view.fit(BLOCK, 640, 480),
                fit_along(view.direction(), view.up(), BLOCK, 640, 480)
            );
        }
        // A needle 500,000 mm long seen end on: 1 mm across, so 1.1 mm by its extent, but
        // its half diagonal's least at 400 rows, 400 · 250,000 / 2^19 · 1.01 = 192.6422 (by
        // hand), binds.
        let needle = [[0.0; 3], [1.0, 500_000.0, 1.0]];
        let (end_on, _) = fit_along([0.0, -1.0, 0.0], [0.0, 0.0, 1.0], needle, 400, 400);
        let Projection::Orthographic { view_height } = end_on.projection else {
            panic!("orthographic")
        };
        assert!((view_height - 192.642_211_9).abs() < 1e-6, "{view_height}");
    }

    #[test]
    fn the_reach_is_the_farthest_corner_and_the_least_height_a_hundredth_over_its_share() {
        // From the origin, the block's farthest corner is (40, 20, 10): √2100.
        assert_eq!(reach([0.0; 3], [BLOCK]), 2100f64.sqrt());
        // From its centre, half its diagonal; a second box farther off wins.
        assert_eq!(reach([20.0, 10.0, 5.0], [BLOCK]), 525f64.sqrt());
        let far = [[100.0, 0.0, 0.0], [101.0, 1.0, 1.0]];
        assert_eq!(
            reach([0.0; 3], [BLOCK, far]),
            (101.0f64 * 101.0 + 2.0).sqrt()
        );
        assert_eq!(reach([0.0; 3], []), 0.0);
        // A box off the origin, (1, 2, 3) to (5, 6, 7), from (4, 4, 4): its farthest
        // corner is 3 back along x, 2 either way along y, and 3 on along z: √22. From
        // (6, 6, 6), at or past its high side in x and y, its low corner is farthest: 5, 4,
        // and 3 back, √50.
        let off = [[1.0, 2.0, 3.0], [5.0, 6.0, 7.0]];
        assert_eq!(reach([4.0; 3], [off]), 22f64.sqrt());
        assert_eq!(reach([6.0; 3], [off]), 50f64.sqrt());
        // By hand: 2^19 mm at 1 row is the renderer's 1 mm, and a hundredth more; at 2048
        // rows, 2048 mm and a hundredth more; 1e5 mm at 1080 rows, 1080 × 1e5 / 2^19 =
        // 205.99365234375 mm (exact), and a hundredth more.
        assert_eq!(least_view_height(1, 524_288.0), 1.01);
        assert_eq!(least_view_height(2048, 524_288.0), 2048.0 * 1.01);
        assert_eq!(least_view_height(1080, 1e5), 205.993_652_343_75 * 1.01);
        assert_eq!(
            joined([BLOCK, far]),
            Some([[0.0, 0.0, 0.0], [101.0, 20.0, 10.0]])
        );
        assert_eq!(joined([]), None);
    }
}
