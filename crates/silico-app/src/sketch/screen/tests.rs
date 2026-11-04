//! The sketch's plane on the screen and back: hand-worked places for a view straight down
//! on XY (orthographic and in perspective), the round trip to 1e-9 for both projections on
//! XY and on a tilted face's frame, and the views and places that have none.

use eframe::egui::{Pos2, Rect, pos2, vec2};
use silico_core::command::SessionId;
use silico_core::kernel::SketchPlane;
use silico_protocol::vec3::{cross, normalize};
use silico_protocol::{Camera, FaceFrame, Projection};

use super::{Placed, Screen};
use crate::sketch::tests::{rectangle, under};
use crate::viewport::ViewFrame;

/// The viewport's top left on the screen, in points, in these tests.
const ORIGIN: Pos2 = pos2(40.0, 30.0);

fn camera(projection: Projection, eye: [f64; 3], target: [f64; 3], up: [f64; 3]) -> Camera {
    Camera {
        projection,
        eye,
        target,
        up,
    }
}

/// Straight down on XY from 100 above the origin, y up the image.
fn down(projection: Projection) -> Camera {
    camera(projection, [0.0, 0.0, 100.0], [0.0; 3], [0.0, 1.0, 0.0])
}

const ORTHOGRAPHIC: Projection = Projection::Orthographic { view_height: 100.0 };
const PERSPECTIVE: Projection = Projection::Perspective {
    field_of_view: 45.0,
};

fn placed(camera: &Camera, size_px: [u32; 2], ppp: f32, plane: SketchPlane) -> Placed {
    Placed::new(camera, size_px, ppp, ORIGIN, plane).unwrap()
}

/// A face's frame tilted every way: its normal along (1, 2, 3), its origin off the model's.
fn tilted() -> SketchPlane {
    let normal = normalize([1.0, 2.0, 3.0]);
    let x = normalize(cross([0.0, 0.0, 1.0], normal));
    let y = cross(normal, x);
    SketchPlane::Face(FaceFrame {
        origin: [5.0, -3.0, 7.0],
        x,
        y,
    })
}

#[track_caller]
fn within(got: [f64; 2], expected: [f64; 2], tolerance: f64) {
    for k in 0..2 {
        assert!(
            (got[k] - expected[k]).abs() <= tolerance,
            "{got:?} against {expected:?}, within {tolerance}"
        );
    }
}

/// `got` as worked by hand, to the renderer's f32 projection: its matrix's entries in f32,
/// a part in 2^22 of the place each way ([`silico_protocol::F32_ROUNDING`]), against the
/// larger of the place and a millimetre.
#[track_caller]
fn by_hand(got: [f64; 2], expected: [f64; 2]) {
    let size = libm::hypot(expected[0], expected[1]).max(1.0);
    within(got, expected, 4.0 * silico_protocol::F32_ROUNDING * size);
}

/// `got` on the screen at `expected`, to a thousandth of a point.
#[track_caller]
fn at_point(got: Option<Pos2>, expected: Pos2) {
    let got = got.unwrap();
    assert!(
        (got - expected).length() < 1e-3,
        "{got:?} against {expected:?}"
    );
}

#[test]
fn an_orthographic_view_down_on_xy_maps_each_pixel_as_worked_by_hand() {
    // 100 mm high over 200 pixels: half a millimetre a pixel, the image's centre (200, 100)
    // over the origin, x right and y up.
    let p = placed(&down(ORTHOGRAPHIC), [400, 200], 2.0, SketchPlane::Xy);
    by_hand(p.on_plane([200.0, 100.0]).unwrap(), [0.0, 0.0]);
    by_hand(p.on_plane([300.0, 100.0]).unwrap(), [50.0, 0.0]);
    by_hand(p.on_plane([200.0, 0.0]).unwrap(), [0.0, 50.0]);
    by_hand(p.on_plane([0.0, 200.0]).unwrap(), [-100.0, -50.0]);
    by_hand(p.pixel_of([25.0, -10.0]).unwrap(), [250.0, 120.0]);
    // On the screen: the pixel at two to a point, from the viewport's top left at (40, 30).
    at_point(p.project([25.0, -10.0]), pos2(165.0, 90.0));
    by_hand(p.to_plane(pos2(165.0, 90.0)).unwrap(), [25.0, -10.0]);
}

#[test]
fn a_perspective_view_down_on_xy_maps_each_pixel_as_worked_by_hand() {
    // 90° high over 200 pixels, 100 above the plane: the top edge's middle, (100, 0), 45°
    // up, meets the plane 100 · tan 45° = 100 mm up; (150, 100), a quarter across, at
    // 100 · tan 45° / 2 = 50 mm. The renderer's f32 projection, to a few parts in 1e7.
    let camera = down(Projection::Perspective {
        field_of_view: 90.0,
    });
    let p = placed(&camera, [200, 200], 1.0, SketchPlane::Xy);
    by_hand(p.on_plane([100.0, 100.0]).unwrap(), [0.0, 0.0]);
    by_hand(p.on_plane([100.0, 0.0]).unwrap(), [0.0, 100.0]);
    by_hand(p.on_plane([150.0, 100.0]).unwrap(), [50.0, 0.0]);
    by_hand(p.pixel_of([0.0, -100.0]).unwrap(), [100.0, 200.0]);
    // A plane nearer the eye shows its places further out: XY moved 50 up, by a face frame.
    let nearer = SketchPlane::Face(FaceFrame {
        origin: [0.0, 0.0, 50.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
    });
    let p = placed(&camera, [200, 200], 1.0, nearer);
    by_hand(p.pixel_of([25.0, 0.0]).unwrap(), [150.0, 100.0]);
}

/// Places of the plane, near and far, both signs, and a fraction.
const PLACES: [[f64; 2]; 6] = [
    [0.0, 0.0],
    [12.5, -7.0],
    [-40.0, 33.3],
    [1e-3, 2e-3],
    [55.456, -48.9],
    [-3.0, -70.0],
];

#[track_caller]
fn round_trips(p: &Placed) {
    for at in PLACES {
        let pixel = p.pixel_of(at).unwrap();
        let back = p.on_plane(pixel).unwrap();
        let tolerance = 1e-9 * libm::hypot(at[0], at[1]).max(1.0);
        within(back, at, tolerance);
    }
}

#[test]
fn a_place_projected_and_met_again_is_where_it_was_to_1e_9_in_perspective_and_orthographic() {
    let size = [1600, 1000];
    let oblique = |projection| {
        camera(
            projection,
            [120.0, -80.0, 90.0],
            [5.0, 0.0, 3.0],
            [0.0, 0.0, 1.0],
        )
    };
    for projection in [ORTHOGRAPHIC, PERSPECTIVE] {
        // Straight down on XY and on the tilted face (XZ and YZ it sees edge on).
        for plane in [SketchPlane::Xy, tilted()] {
            round_trips(&placed(&down(projection), size, 2.0, plane));
        }
        for plane in [SketchPlane::Xy, SketchPlane::Xz, SketchPlane::Yz, tilted()] {
            round_trips(&placed(&oblique(projection), size, 2.0, plane));
            round_trips(&placed(&oblique(projection), size, 1.25, plane));
        }
    }
}

#[test]
fn on_the_screen_a_place_comes_back_to_the_f32_of_its_point() {
    // Through egui's points, in f32: back to what a point's step there is on the plane.
    let camera = camera(
        PERSPECTIVE,
        [120.0, -80.0, 90.0],
        [5.0, 0.0, 3.0],
        [0.0, 0.0, 1.0],
    );
    let p = placed(&camera, [1600, 1000], 2.0, tilted());
    for at in PLACES {
        let on = p.project(at).unwrap();
        let back = p.to_plane(on).unwrap();
        // A point's step at (1000, 1000) points is 2^-14; the plane's millimetres a point
        // here are under one.
        within(back, at, 1e-3);
    }
}

#[test]
fn a_view_edge_on_to_the_plane_meets_it_nowhere_along_its_middle() {
    // Looking along +y at XY, orthographic: every ray runs along the plane.
    let edge_on = camera(ORTHOGRAPHIC, [0.0, -100.0, 0.0], [0.0; 3], [0.0, 0.0, 1.0]);
    let p = placed(&edge_on, [400, 200], 2.0, SketchPlane::Xy);
    for pixel in [[200.0, 100.0], [0.0, 0.0], [399.0, 13.0]] {
        assert_eq!(p.on_plane(pixel), None, "{pixel:?}");
    }
    // The plane is still drawn, as the line across the image's middle.
    by_hand(p.pixel_of([30.0, 0.0]).unwrap(), [260.0, 100.0]);
    by_hand(p.pixel_of([30.0, 40.0]).unwrap(), [260.0, 100.0]);
    // In perspective with the eye in the plane: each ray meets it at the eye or not at all.
    let eye_in = camera(PERSPECTIVE, [0.0, -100.0, 0.0], [0.0; 3], [0.0, 0.0, 1.0]);
    let p = placed(&eye_in, [400, 200], 2.0, SketchPlane::Xy);
    for pixel in [[200.0, 100.0], [0.0, 0.0], [200.0, 199.0]] {
        assert_eq!(p.on_plane(pixel), None, "{pixel:?}");
    }
    // A screen with no ray at its middle has no scale for the tools there.
    let screen = Screen::Camera(Box::new(placed(&edge_on, [400, 200], 2.0, SketchPlane::Xy)));
    let to_plane = |at| screen.to_plane(at);
    assert_eq!(crate::sketch::scale_at(&to_plane, pos2(140.0, 80.0)), None);
}

#[test]
fn in_perspective_a_place_behind_the_eye_has_no_pixel_and_a_ray_meets_nothing_behind_it() {
    // Eye 100 above XY, looking down past it: the plane z = 150 is behind it.
    let behind = SketchPlane::Face(FaceFrame {
        origin: [0.0, 0.0, 150.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
    });
    let p = placed(&down(PERSPECTIVE), [400, 200], 2.0, behind);
    assert_eq!(p.pixel_of([0.0, 0.0]), None);
    assert_eq!(p.pixel_of([10.0, -5.0]), None);
    assert_eq!(p.on_plane([200.0, 100.0]), None);
    assert_eq!(p.project([0.0, 0.0]), None);
    // The eye's own height has no pixel either.
    let level = SketchPlane::Face(FaceFrame {
        origin: [0.0, 0.0, 100.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
    });
    let p = placed(&down(PERSPECTIVE), [400, 200], 2.0, level);
    assert_eq!(p.pixel_of([10.0, 0.0]), None);
    // Orthographic, the same plane in front of the target is drawn, and met.
    let p = placed(&down(ORTHOGRAPHIC), [400, 200], 2.0, behind);
    by_hand(p.pixel_of([10.0, 0.0]).unwrap(), [220.0, 100.0]);
    by_hand(p.on_plane([220.0, 100.0]).unwrap(), [10.0, 0.0]);
}

#[test]
fn a_pointer_off_the_viewport_still_meets_the_plane_where_its_ray_does() {
    let p = placed(&down(ORTHOGRAPHIC), [400, 200], 2.0, SketchPlane::Xy);
    // 500 pixels left of the image and 3,000 below: 250 mm left, 1,500 down, of its edge.
    by_hand(p.on_plane([-500.0, 3200.0]).unwrap(), [-350.0, -1550.0]);
    // And off the screen's points too, unclamped: (-1000, 2000) is 2,080 pixels left of the
    // image's left and 3,940 below its top.
    by_hand(
        p.to_plane(pos2(-1000.0, 2000.0)).unwrap(),
        [-1140.0, -1920.0],
    );
}

#[test]
fn nan_and_infinite_places_and_pixels_give_none() {
    let p = placed(&down(PERSPECTIVE), [400, 200], 2.0, SketchPlane::Xy);
    for at in [
        [f64::NAN, 0.0],
        [0.0, f64::INFINITY],
        [f64::NEG_INFINITY, 1.0],
    ] {
        assert_eq!(p.pixel_of(at), None, "{at:?}");
        assert_eq!(p.project(at), None, "{at:?}");
    }
    for pixel in [[f64::NAN, 0.0], [0.0, f64::INFINITY]] {
        assert_eq!(p.on_plane(pixel), None, "{pixel:?}");
    }
    assert_eq!(p.to_plane(pos2(f32::NAN, 10.0)), None);
    assert_eq!(p.to_plane(pos2(10.0, f32::INFINITY)), None);
    // A place so far its pixel passes f32 isn't put on the screen.
    let p = placed(&down(ORTHOGRAPHIC), [400, 200], 2.0, SketchPlane::Xy);
    assert_eq!(p.project([1e300, 0.0]), None);
}

#[test]
fn a_camera_or_scale_the_renderer_refuses_places_nothing() {
    let good = down(PERSPECTIVE);
    let plane = SketchPlane::Xy;
    assert!(Placed::new(&good, [0, 200], 2.0, ORIGIN, plane).is_none());
    assert!(Placed::new(&good, [400, 0], 2.0, ORIGIN, plane).is_none());
    for ppp in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(Placed::new(&good, [400, 200], ppp, ORIGIN, plane).is_none());
    }
    // An eye on its target, and an up along the view.
    let on_target = camera(PERSPECTIVE, [0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]);
    assert!(Placed::new(&on_target, [400, 200], 2.0, ORIGIN, plane).is_none());
    let up_along = camera(PERSPECTIVE, [0.0, 0.0, 100.0], [0.0; 3], [0.0, 0.0, 1.0]);
    assert!(Placed::new(&up_along, [400, 200], 2.0, ORIGIN, plane).is_none());
    let nan_eye = camera(
        PERSPECTIVE,
        [f64::NAN, 0.0, 100.0],
        [0.0; 3],
        [0.0, 1.0, 0.0],
    );
    assert!(Placed::new(&nan_eye, [400, 200], 2.0, ORIGIN, plane).is_none());
}

fn frame(camera: Camera) -> ViewFrame {
    ViewFrame {
        session: SessionId::parse("s1").unwrap(),
        rect: Rect::from_min_size(ORIGIN, vec2(200.0, 100.0)),
        camera,
        size_px: [400, 200],
        pixels_per_point: 2.0,
        pointer_px: None,
        left: Vec::new(),
        secondary: Vec::new(),
    }
}

#[test]
fn a_sketch_with_its_plane_is_placed_through_the_frames_camera_and_a_broken_face_flat() {
    let read = under(rectangle(Vec::new()));
    let shown = frame(down(ORTHOGRAPHIC));
    let screen = Screen::of(&read, &shown).unwrap();
    assert!(matches!(screen, Screen::Camera(_)));
    // As by hand: the image's centre, (140, 80) on the screen, over the origin; half a
    // millimetre a pixel, a millimetre a point.
    at_point(screen.project([0.0, 0.0]), pos2(140.0, 80.0));
    at_point(screen.project([40.0, 20.0]), pos2(180.0, 60.0));
    by_hand(screen.to_plane(pos2(180.0, 60.0)).unwrap(), [40.0, 20.0]);
    // Its face broken: flat in the frame's rectangle, as `Flat::fit` fits it (the rectangle
    // from (0, 0) to (40, 20) over 80 of the 100 points high: two points a millimetre,
    // centred at (140, 80)).
    let mut broken = read.clone();
    broken.plane = Err(silico_core::Error::new(
        silico_core::ErrorCode::ReferenceBroken,
        "the face broke",
    ));
    let screen = Screen::of(&broken, &shown).unwrap();
    assert!(matches!(screen, Screen::Flat(_)));
    assert_eq!(screen.project([20.0, 10.0]), Some(pos2(140.0, 80.0)));
    assert_eq!(screen.project([0.0, 0.0]), Some(pos2(100.0, 100.0)));
    assert_eq!(screen.to_plane(pos2(100.0, 100.0)), Some([0.0, 0.0]));
    assert_eq!(screen.project([f64::NAN, 0.0]), None);
    assert_eq!(screen.to_plane(pos2(f32::NAN, 0.0)), None);
    // A camera the renderer refuses: nothing placed, whatever the plane.
    let refused = frame(camera(ORTHOGRAPHIC, [0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]));
    assert!(Screen::of(&read, &refused).is_none());
}
