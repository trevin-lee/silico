//! The named views a render may ask for, and the camera each makes of the bodies in view
//! (docs/design/m4.6-render.md, *Named views*): Fusion's ViewCube's six faces and eight
//! corners, in the document's Z-up axes (FreeCAD's), every one orthographic and fitted to
//! the bodies' tight box. Computed in f64, in the order the design writes it, with no
//! trigonometry, so a view's camera is the same on every platform: it's a result.

use silico_protocol::{Camera, Projection};

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
}

/// Every named view, in the order above.
pub const NAMED_VIEWS: [NamedView; 14] = [
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
];

impl NamedView {
    /// Its name, as a request spells it: `front`, `top_front_right`, ...
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
        }
    }

    /// The view named `name`, if there's one.
    pub fn named(name: &str) -> Option<NamedView> {
        NAMED_VIEWS.into_iter().find(|v| v.name() == name)
    }

    /// The direction from the target to the eye, a unit vector: a face's axis, or a
    /// corner's diagonal, whose 1/√3 is `1.0 / 3.0f64.sqrt()` (`right` is +x, `back` +y,
    /// `top` +z).
    pub fn direction(self) -> [f64; 3] {
        let d = 1.0 / 3.0f64.sqrt();
        let corner = |x: f64, y: f64, z: f64| [x * d, y * d, z * d];
        match self {
            NamedView::Front => [0.0, -1.0, 0.0],
            NamedView::Back => [0.0, 1.0, 0.0],
            NamedView::Left => [-1.0, 0.0, 0.0],
            NamedView::Right => [1.0, 0.0, 0.0],
            NamedView::Top => [0.0, 0.0, 1.0],
            NamedView::Bottom => [0.0, 0.0, -1.0],
            NamedView::TopFrontLeft => corner(-1.0, -1.0, 1.0),
            NamedView::TopFrontRight => corner(1.0, -1.0, 1.0),
            NamedView::TopBackLeft => corner(-1.0, 1.0, 1.0),
            NamedView::TopBackRight => corner(1.0, 1.0, 1.0),
            NamedView::BottomFrontLeft => corner(-1.0, -1.0, -1.0),
            NamedView::BottomFrontRight => corner(1.0, -1.0, -1.0),
            NamedView::BottomBackLeft => corner(-1.0, 1.0, -1.0),
            NamedView::BottomBackRight => corner(1.0, 1.0, -1.0),
        }
    }

    /// Which way is up in the image, a unit vector across the view: +z for the side views;
    /// +y for `top` and `bottom` (so `top` shows the front at the image's bottom, as Fusion's
    /// and FreeCAD's do); for a corner, +z made perpendicular to its direction.
    pub fn up(self) -> [f64; 3] {
        match self {
            NamedView::Front | NamedView::Back | NamedView::Left | NamedView::Right => {
                [0.0, 0.0, 1.0]
            }
            NamedView::Top | NamedView::Bottom => [0.0, 1.0, 0.0],
            _ => {
                let dir = self.direction();
                let z = [0.0, 0.0, 1.0];
                let along = dot(z, dir);
                let up = [
                    z[0] - along * dir[0],
                    z[1] - along * dir[1],
                    z[2] - along * dir[2],
                ];
                let length = dot(up, up).sqrt();
                [up[0] / length, up[1] / length, up[2] / length]
            }
        }
    }

    /// The camera that fits a tight box, `[low, high]`, into an image `width` by `height`:
    /// orthographic, looking at the box's centre from `direction`, its eye a diagonal of
    /// the box away (outside every body, so the near plane cuts nothing), and its view
    /// height 1.1 times the larger of the box's corners' extent along `up` and their extent
    /// along the image's right (`up × direction`) times `height / width`. So the bodies fill
    /// the image with a twentieth of it to spare on each side of the tighter direction.
    pub fn camera(self, [low, high]: [[f64; 3]; 2], width: u32, height: u32) -> Camera {
        let dir = self.direction();
        let up = self.up();
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
        let view_height = 1.1 * extent_up.max(extent_right);
        let half_diagonal = {
            let d = [high[0] - low[0], high[1] - low[1], high[2] - low[2]];
            dot(d, d).sqrt() / 2.0
        };
        let eye = [
            target[0] + dir[0] * 2.0 * half_diagonal,
            target[1] + dir[1] * 2.0 * half_diagonal,
            target[2] + dir[2] * 2.0 * half_diagonal,
        ];
        Camera {
            projection: Projection::Orthographic { view_height },
            eye,
            target,
            up,
        }
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::{NAMED_VIEWS, NamedView, cross, dot};
    use silico_protocol::Projection;

    const BLOCK: [[f64; 3]; 2] = [[0.0, 0.0, 0.0], [40.0, 20.0, 10.0]];

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
                    // A corner's name says its direction's signs; its up has +z.
                    let name = view.name();
                    let sign = |yes: &str, no: &str| {
                        if name.contains(yes) {
                            1.0
                        } else {
                            assert!(name.contains(no));
                            -1.0
                        }
                    };
                    let want = [
                        sign("right", "left"),
                        sign("back", "front"),
                        sign("top", "bottom"),
                    ];
                    for k in 0..3 {
                        assert_eq!(dir[k].signum(), want[k], "{view:?}");
                    }
                    assert!(up[2] > 0.0, "{view:?}");
                }
            }
            assert_eq!(NamedView::named(view.name()), Some(view));
        }
        assert_eq!(NamedView::named("front_top"), None);
        assert_eq!(NamedView::named("Front"), None);
    }

    #[test]
    fn every_view_fits_the_box_with_a_twentieth_to_spare_on_its_tighter_side() {
        for (width, height) in [(800, 600), (300, 900), (1, 1), (4096, 1)] {
            for view in NAMED_VIEWS {
                let camera = view.camera(BLOCK, width, height);
                camera.check().unwrap();
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
}
