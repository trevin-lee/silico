//! The triad and the view label, worked by hand from each camera: its right, up, and back
//! (the eye's way from the target), each axis's end the origin plus 26 times its parts along
//! the right and up (the screen's y down), and the label's words as the prototype writes them.

use eframe::egui::epaint::TextShape;
use eframe::egui::{self, Color32, Pos2, Rect, Shape, pos2, vec2};
use silico_protocol::{Camera, Projection};

use super::{Triad, baseline, triad, view_label, view_name};

/// The canvas: 800 by 600 points from (100, 50), so the triad's origin is at (144, 606), 44
/// in from its left and up from its bottom (650), and the label's line's bottom left at
/// (182, 638), 82 in and 12 up.
fn canvas() -> Rect {
    Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0))
}

const ORIGIN: Pos2 = pos2(144.0, 606.0);

/// Orthographic, 80 mm high, from `toward` (scaled to 100 mm out) onto the origin, `up` up.
fn looking(toward: [f64; 3], up: [f64; 3]) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: toward.map(|c| c * 100.0),
        target: [0.0; 3],
        up,
    }
}

/// [`looking`] in perspective, at 30°.
fn in_perspective(toward: [f64; 3], up: [f64; 3]) -> Camera {
    Camera {
        projection: Projection::Perspective {
            field_of_view: 30.0,
        },
        ..looking(toward, up)
    }
}

fn front() -> Camera {
    looking([0.0, -1.0, 0.0], [0.0, 0.0, 1.0])
}

fn top() -> Camera {
    looking([0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
}

/// The home view's corner: from (1, −1, 1), its up z made square to it, (−1, 1, 2)/√6.
fn iso() -> Camera {
    let s = 6f64.sqrt();
    looking([1.0, -1.0, 1.0], [-1.0 / s, 1.0 / s, 2.0 / s])
}

#[track_caller]
fn at(got: Pos2, want: Pos2) {
    assert!((got - want).length() < 1e-4, "{got:?} against {want:?}");
}

/// The axes' letters, in the order drawn.
fn order(triad: &Triad) -> Vec<&'static str> {
    triad.axes.iter().map(|axis| axis.letter).collect()
}

/// The axis lettered `letter`.
fn axis(triad: &Triad, letter: &str) -> super::TriadAxis {
    *triad
        .axes
        .iter()
        .find(|axis| axis.letter == letter)
        .unwrap()
}

#[test]
fn from_the_front_x_points_right_z_up_and_y_away_from_the_viewer_with_no_length() {
    // From −y, z up: right = up × back = z × (−y) = x. So X runs 26 right, Z 26 up (the
    // screen's y less), and Y, along the view, is the origin itself; its letter too. Y
    // points straight away (back · y = −1), X and Z across (0): Y drawn first, then X and Z
    // in their order, Z last, on top.
    let t = triad(&front(), canvas()).unwrap();
    assert_eq!(t.origin, ORIGIN);
    at(axis(&t, "X").end, pos2(170.0, 606.0));
    at(axis(&t, "X").letter_at, pos2(179.0, 606.0));
    at(axis(&t, "Z").end, pos2(144.0, 580.0));
    at(axis(&t, "Z").letter_at, pos2(144.0, 571.0));
    assert_eq!(axis(&t, "Y").end, ORIGIN);
    assert_eq!(axis(&t, "Y").letter_at, ORIGIN);
    assert_eq!(order(&t), ["Y", "X", "Z"]);
    assert_eq!(
        t.axes.map(|axis| axis.toward),
        [-1.0, 0.0, 0.0],
        "{:?}",
        t.axes
    );
    // Each in its colour: `#ff6981`, `#00b266`, `#7a95ff`.
    assert_eq!(axis(&t, "X").colour, Color32::from_rgb(0xff, 0x69, 0x81));
    assert_eq!(axis(&t, "Y").colour, Color32::from_rgb(0x00, 0xb2, 0x66));
    assert_eq!(axis(&t, "Z").colour, Color32::from_rgb(0x7a, 0x95, 0xff));
}

#[test]
fn from_the_top_x_points_right_y_up_and_z_at_the_viewer_drawn_last() {
    // From +z, y up: right = y × z = x. X 26 right, Y 26 up, Z along the view (back · z = 1),
    // nearest the viewer: drawn last.
    let t = triad(&top(), canvas()).unwrap();
    at(axis(&t, "X").end, pos2(170.0, 606.0));
    at(axis(&t, "Y").end, pos2(144.0, 580.0));
    at(axis(&t, "Y").letter_at, pos2(144.0, 571.0));
    assert_eq!(axis(&t, "Z").end, ORIGIN);
    assert_eq!(order(&t), ["X", "Y", "Z"]);
}

#[test]
fn from_the_iso_corner_each_axis_is_as_long_and_their_lines_are_120_degrees_apart() {
    // From (1, −1, 1)/√3, up (−1, 1, 2)/√6: right = up × back = (3, 3, 0)/√18 = (1, 1, 0)/√2.
    // So on the screen (y up), X is (1/√2, −1/√6), Y (1/√2, 1/√6), and Z (0, 2/√6): each
    // √(2/3) long, 26√(2/3) = 21.229 points, X at −30°, Y at +30°, Z at 90°. X and Z point
    // toward the viewer (1/√3), Y away (−1/√3): so the lines X, Z, and Y's far side (at
    // 210°) are 120° apart, as an isometric view's axes are. Y is drawn first, under them;
    // X then Z, as far toward the viewer, in their order.
    let t = triad(&iso(), canvas()).unwrap();
    let (r2, r6) = (2f32.sqrt(), 6f32.sqrt());
    at(axis(&t, "X").end, ORIGIN + vec2(26.0 / r2, 26.0 / r6));
    at(axis(&t, "Y").end, ORIGIN + vec2(26.0 / r2, -26.0 / r6));
    at(axis(&t, "Z").end, ORIGIN + vec2(0.0, -52.0 / r6));
    at(axis(&t, "Z").letter_at, ORIGIN + vec2(0.0, -70.0 / r6));
    let length = 26.0 * (2.0f32 / 3.0).sqrt();
    let angle = |letter: &str| {
        let d = axis(&t, letter).end - ORIGIN;
        assert!((d.length() - length).abs() < 1e-4, "{letter}: {d:?}");
        // Counter-clockwise from the screen's right, its y up.
        libm::atan2f(-d.y, d.x).to_degrees()
    };
    assert!((angle("X") + 30.0).abs() < 1e-3, "{}", angle("X"));
    assert!((angle("Y") - 30.0).abs() < 1e-3, "{}", angle("Y"));
    assert!((angle("Z") - 90.0).abs() < 1e-3, "{}", angle("Z"));
    let third = 1.0 / 3f64.sqrt();
    let toward = |letter: &str| axis(&t, letter).toward;
    assert!((toward("X") - third).abs() < 1e-15);
    assert!((toward("Y") + third).abs() < 1e-15);
    assert!((toward("Z") - third).abs() < 1e-15);
    assert_eq!(order(&t), ["Y", "X", "Z"]);
}

#[test]
fn the_triad_turns_with_the_view_but_keeps_its_size_whatever_the_zoom_or_projection() {
    // Directions only: 8 mm high or 8 m, in perspective or not, the same triad.
    let near = Camera {
        projection: Projection::Orthographic { view_height: 8.0 },
        ..front()
    };
    let far = Camera {
        projection: Projection::Orthographic {
            view_height: 8000.0,
        },
        ..front()
    };
    let want = triad(&front(), canvas());
    assert_eq!(triad(&near, canvas()), want);
    assert_eq!(triad(&far, canvas()), want);
    assert_eq!(
        triad(&in_perspective([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]), canvas()),
        want
    );
    // From the back (+y), x points left: right = z × y = −x.
    let back = triad(&looking([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]), canvas()).unwrap();
    at(axis(&back, "X").end, pos2(118.0, 606.0));
}

#[test]
fn an_eye_not_at_one_axis_tilts_the_one_along_the_view_to_a_short_stub() {
    // From the front tilted 30° up, (0, −cos 30°, sin 30°), up (0, sin 30°, cos 30°): right
    // still x; y's part along up is sin 30° = 0.5, so Y runs 13 up the screen and Z
    // 26 cos 30° = 22.517 up; Y points away by cos 30°, Z toward by sin 30°.
    let (s, c) = (0.5f64, 3f64.sqrt() / 2.0);
    let t = triad(&looking([0.0, -c, s], [0.0, s, c]), canvas()).unwrap();
    at(axis(&t, "Y").end, pos2(144.0, 606.0 - 13.0));
    at(axis(&t, "Z").end, pos2(144.0, 606.0 - 26.0 * c as f32));
    assert_eq!(order(&t), ["Y", "X", "Z"]);
}

/// `f` with a painter, in one pass of a context with the app's fonts.
fn with_painter(f: impl FnOnce(&egui::Painter)) {
    with_painter_at(1.0, f);
}

/// [`with_painter`] at `pixels_per_point`: text laid out, and its metrics rounded, to that
/// many pixels a point.
fn with_painter_at(pixels_per_point: f32, f: impl FnOnce(&egui::Painter)) {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::fonts());
    ctx.set_pixels_per_point(pixels_per_point);
    let mut f = Some(f);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(f) = f.take() {
            f(&ui.painter().clone());
        }
    });
    output.textures_delta.clear();
}

/// A text shape's middle across and its baseline.
fn text_place(text: &TextShape) -> Pos2 {
    pos2(
        text.pos.x + text.galley.size().x / 2.0,
        text.pos.y + baseline(&text.galley),
    )
}

#[test]
fn the_triads_shapes_are_each_axis_farthest_first_its_stroke_cap_and_letter_then_the_dot() {
    // From the front: Y (along the view) its letter alone, at the origin; then X's 2-wide
    // stroke from the origin to (170, 606), its round cap there (a disc of radius 1), and its
    // letter "X" 10.5 high, its middle at 179 and its baseline 3.5 below 606; then Z's, to
    // (144, 580), its letter at 144, its baseline at 571 + 3.5; last the origin's dot, radius
    // 2, #cfcfcf.
    with_painter(|painter| {
        let shapes = triad(&front(), canvas()).unwrap().shapes(painter);
        assert_eq!(shapes.len(), 1 + 3 + 3 + 1, "{shapes:?}");
        let letter = |shape: &Shape, want: &str, colour: Color32, place: Pos2| {
            let Shape::Text(text) = shape else {
                panic!("{shape:?}");
            };
            assert_eq!(text.galley.text(), want);
            assert_eq!(text.galley.job.sections[0].format.font_id.size, 10.5);
            assert_eq!(text.galley.job.sections[0].format.color, colour);
            at(text_place(text), place);
        };
        let x = Color32::from_rgb(0xff, 0x69, 0x81);
        let y = Color32::from_rgb(0x00, 0xb2, 0x66);
        let z = Color32::from_rgb(0x7a, 0x95, 0xff);
        letter(&shapes[0], "Y", y, pos2(144.0, 609.5));
        let stroke = |shape: &Shape, end: Pos2, colour: Color32| {
            let Shape::LineSegment { points, stroke } = shape else {
                panic!("{shape:?}");
            };
            assert_eq!(points[0], ORIGIN);
            at(points[1], end);
            assert_eq!((stroke.width, stroke.color), (2.0, colour));
        };
        let cap = |shape: &Shape, end: Pos2, colour: Color32| {
            let Shape::Circle(disc) = shape else {
                panic!("{shape:?}");
            };
            at(disc.center, end);
            assert_eq!((disc.radius, disc.fill), (1.0, colour));
        };
        stroke(&shapes[1], pos2(170.0, 606.0), x);
        cap(&shapes[2], pos2(170.0, 606.0), x);
        letter(&shapes[3], "X", x, pos2(179.0, 609.5));
        stroke(&shapes[4], pos2(144.0, 580.0), z);
        cap(&shapes[5], pos2(144.0, 580.0), z);
        letter(&shapes[6], "Z", z, pos2(144.0, 574.5));
        let Shape::Circle(dot) = &shapes[7] else {
            panic!("{:?}", shapes[7]);
        };
        assert_eq!(dot.center, ORIGIN);
        assert_eq!(dot.radius, 2.0);
        assert_eq!(dot.fill, Color32::from_rgb(0xcf, 0xcf, 0xcf));
    });
}

#[test]
fn each_letters_baseline_is_3_5_below_its_place_at_every_scale_not_its_middle() {
    // SVG's `<text y>` is the baseline: each letter's baseline 3.5 below `letter_at`, its
    // middle across at it, whatever the font's line box at 1, 1.5, 2, or 3 pixels a point
    // (where centring the line box on the place would land its baseline elsewhere). From the
    // front: X's at (179, 609.5), Z's at (144, 574.5), Y's at the origin's (144, 609.5).
    for ppp in [1.0, 1.5, 2.0, 3.0] {
        with_painter_at(ppp, |painter| {
            let shapes = triad(&front(), canvas()).unwrap().shapes(painter);
            let letters: Vec<(String, Pos2)> = shapes
                .iter()
                .filter_map(|shape| match shape {
                    Shape::Text(text) => Some((text.galley.text().to_owned(), text_place(text))),
                    _ => None,
                })
                .collect();
            let want = [
                ("Y", pos2(144.0, 609.5)),
                ("X", pos2(179.0, 609.5)),
                ("Z", pos2(144.0, 574.5)),
            ];
            assert_eq!(letters.len(), 3, "{ppp}: {letters:?}");
            for ((letter, place), (wanted, at_place)) in letters.iter().zip(want) {
                assert_eq!(letter, wanted, "{ppp}");
                at(*place, at_place);
            }
        });
    }
}

#[test]
fn the_view_label_is_the_projection_and_the_named_view_the_prototype_names() {
    let r = std::f64::consts::FRAC_1_SQRT_2;
    let z = [0.0, 0.0, 1.0];
    for (camera, words) in [
        (front(), "Orthographic · Front"),
        (looking([0.0, 1.0, 0.0], z), "Orthographic · Back"),
        (looking([-1.0, 0.0, 0.0], z), "Orthographic · Left"),
        (looking([1.0, 0.0, 0.0], z), "Orthographic · Right"),
        (top(), "Orthographic · Top"),
        (
            looking([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
            "Orthographic · Bottom",
        ),
        (iso(), "Orthographic · Iso"),
        (in_perspective([0.0, -1.0, 0.0], z), "Perspective · Front"),
        // The ViewCube's other corners, which the prototype's `VIEWS` doesn't hold: the
        // projection alone. Top front left: from (−1, −1, 1), up (1, 1, 2)/√6.
        (
            looking(
                [-1.0, -1.0, 1.0],
                [1.0 / 6f64.sqrt(), 1.0 / 6f64.sqrt(), 2.0 / 6f64.sqrt()],
            ),
            "Orthographic",
        ),
        // A ViewCube edge's view, between the top and the front: no named view.
        (looking([0.0, -r, r], [0.0, r, r]), "Orthographic"),
    ] {
        assert_eq!(view_label(&camera, canvas()).words, words, "{camera:?}");
    }
    assert_eq!(
        view_label(&front(), canvas()).bottom_left,
        pos2(182.0, 638.0)
    );
}

#[test]
fn a_view_is_named_within_a_hundredth_of_a_radian_of_its_direction_and_its_up() {
    // The prototype's `viewName`: within 0.01 of the named view. Turned about z by 0.0099
    // from the front, still Front; by 0.0101, not. Tilted up as much, the same; rolled about
    // the view (its up turned in the screen's plane) as much, the same.
    for (angle, named) in [(0.0099f64, true), (0.0101, false)] {
        let (s, c) = (libm::sin(angle), libm::cos(angle));
        let turned = looking([s, -c, 0.0], [0.0, 0.0, 1.0]);
        let tilted = looking([0.0, -c, s], [0.0, s, c]);
        let rolled = looking([0.0, -1.0, 0.0], [s, 0.0, c]);
        for camera in [turned, tilted, rolled] {
            assert_eq!(
                view_name(&camera).as_deref(),
                named.then_some("Front"),
                "{angle}: {camera:?}"
            );
        }
    }
    // Upside down from the back (+y, up −z), as an orbit over the top leaves it: not Back.
    assert_eq!(view_name(&looking([0.0, 1.0, 0.0], [0.0, 0.0, -1.0])), None);
    // Straight down whatever its up, as the prototype takes the top at any azimuth: Top.
    for up in [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.6, 0.8, 0.0]] {
        assert_eq!(
            view_name(&looking([0.0, 0.0, 1.0], up)).as_deref(),
            Some("Top")
        );
        assert_eq!(
            view_name(&looking([0.0, 0.0, -1.0], up)).as_deref(),
            Some("Bottom")
        );
    }
}

#[test]
fn an_orbit_of_a_point_keeps_the_name_and_of_two_drops_it() {
    // Half a degree a point (`navigate::ORBIT_DEGREES_PER_POINT`): one point turns the view
    // 0.5° = 0.008727 rad, inside a hundredth; two turn it 1° = 0.017453, outside.
    let one = crate::navigate::orbit(&front(), 1.0, 0.0).unwrap();
    let two = crate::navigate::orbit(&front(), 2.0, 0.0).unwrap();
    assert_eq!(view_name(&one).as_deref(), Some("Front"));
    assert_eq!(view_name(&two), None);
    let up_two = crate::navigate::orbit(&front(), 0.0, 2.0).unwrap();
    assert_eq!(view_name(&up_two), None);
}

#[test]
fn the_view_labels_shape_is_its_words_on_its_16_point_line_from_its_left() {
    // 11 points high in #827f7f, from x = 182, its line from 622 to 638, the words' box
    // centred down it.
    with_painter(|painter| {
        let label = view_label(&iso(), canvas());
        let (shape, line) = label.shape(painter);
        let Shape::Text(text) = shape else {
            panic!("{shape:?}");
        };
        assert_eq!(text.galley.text(), "Orthographic · Iso");
        let format = &text.galley.job.sections[0].format;
        assert_eq!(format.font_id.size, 11.0);
        assert_eq!(format.color, Color32::from_rgb(0x82, 0x7f, 0x7f));
        assert_eq!(line.min, pos2(182.0, 622.0));
        assert_eq!(line.max, pos2(182.0 + text.galley.size().x, 638.0));
        assert_eq!(text.pos.x, 182.0);
        let middle = text.pos.y + text.galley.size().y / 2.0;
        assert!((middle - 630.0).abs() < 1e-4, "{middle}");
    });
}

// ---- Hostile input ----

#[test]
fn a_camera_that_makes_no_frame_draws_no_triad_and_names_no_view() {
    // Its eye at its target, or a NaN in it: no right or up to draw by. The label still says
    // the projection, which doesn't need one.
    let at_target = looking([0.0; 3], [0.0, 0.0, 1.0]);
    let nan = looking([f64::NAN, -1.0, 0.0], [0.0, 0.0, 1.0]);
    let along = looking([0.0, -1.0, 0.0], [0.0, -1.0, 0.0]);
    for camera in [at_target, nan, along] {
        assert_eq!(triad(&camera, canvas()), None, "{camera:?}");
        assert_eq!(view_name(&camera), None);
        assert_eq!(view_label(&camera, canvas()).words, "Orthographic");
    }
}

#[test]
fn a_canvas_of_no_size_still_places_the_triad_by_its_corner_and_one_nowhere_draws_none() {
    // No size at (0, 0): the origin at (44, −44), off the canvas, which the pane's clip
    // leaves undrawn, as the prototype's `overflow: hidden` does. A canvas whose corners
    // aren't finite (egui's `Rect::NOTHING`, or a NaN): no triad, and the label's place not
    // finite either, but its words the same.
    let t = triad(&front(), Rect::ZERO).unwrap();
    assert_eq!(t.origin, pos2(44.0, -44.0));
    at(axis(&t, "X").end, pos2(70.0, -44.0));
    assert_eq!(
        view_label(&front(), Rect::ZERO).bottom_left,
        pos2(82.0, -12.0)
    );
    for rect in [
        Rect::NOTHING,
        Rect::from_min_size(pos2(f32::NAN, 0.0), vec2(10.0, 10.0)),
    ] {
        assert_eq!(triad(&front(), rect), None, "{rect:?}");
        assert_eq!(view_label(&front(), rect).words, "Orthographic · Front");
    }
}

#[test]
fn an_up_all_but_along_the_view_turns_the_triad_by_what_is_left_of_it_across() {
    // From the front, its up (0, 1, 2e-9): along the view but for 2e-9 of z, over the least
    // a frame takes across (1e-9). Made square to the view, the up is z: the front's triad,
    // and its name. Its up (2e-9, 1, 0) instead: the up is x, so right = x × (−y) = −z: X
    // runs 26 up the screen, Z 26 left, and the view is the front rolled a quarter turn,
    // which no named view is.
    let nearly_z = looking([0.0, -1.0, 0.0], [0.0, 1.0, 2e-9]);
    assert_eq!(triad(&nearly_z, canvas()), triad(&front(), canvas()));
    assert_eq!(view_name(&nearly_z).as_deref(), Some("Front"));
    let nearly_x = looking([0.0, -1.0, 0.0], [2e-9, 1.0, 0.0]);
    let t = triad(&nearly_x, canvas()).unwrap();
    at(axis(&t, "X").end, pos2(144.0, 580.0));
    at(axis(&t, "Z").end, pos2(118.0, 606.0));
    assert_eq!(axis(&t, "Y").end, ORIGIN);
    assert_eq!(view_name(&nearly_x), None);
}
