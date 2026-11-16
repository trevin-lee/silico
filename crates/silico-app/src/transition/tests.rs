//! The view transition's ease and path, worked by hand: the prototype's ease at its quarters,
//! the azimuth's short way round, and each camera between from its azimuth, elevation, roll,
//! target, distance, and height moved by hand.

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use silico_protocol::{Camera, Projection};

use super::{Motion, Step, Transition, between, ease, short_turn};

#[track_caller]
fn near(a: [f64; 3], b: [f64; 3], within: f64) {
    for k in 0..3 {
        assert!((a[k] - b[k]).abs() <= within, "{a:?} against {b:?}");
    }
}

#[track_caller]
fn near1(a: f64, b: f64, within: f64) {
    assert!((a - b).abs() <= within, "{a} against {b}");
}

fn height(camera: &Camera) -> f64 {
    match camera.projection {
        Projection::Orthographic { view_height } => view_height,
        Projection::Perspective { .. } => f64::NAN,
    }
}

/// Orthographic, `view_height` high, onto `target` from `target + eye`, `up` up.
fn camera(view_height: f64, target: [f64; 3], eye: [f64; 3], up: [f64; 3]) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height },
        eye: std::array::from_fn(|k| target[k] + eye[k]),
        target,
        up,
    }
}

/// From −y, 100 mm out, onto the origin, 80 high: azimuth 0, elevation 0.
fn front() -> Camera {
    camera(80.0, [0.0; 3], [0.0, -100.0, 0.0], [0.0, 0.0, 1.0])
}

/// From the azimuth `degrees` (from −y toward +x), level, 100 mm out onto the origin.
fn level_at(degrees: f64) -> Camera {
    let a = degrees.to_radians();
    camera(
        80.0,
        [0.0; 3],
        [100.0 * libm::sin(a), -100.0 * libm::cos(a), 0.0],
        [0.0, 0.0, 1.0],
    )
}

#[test]
fn the_ease_is_the_prototypes_quadratic_in_and_out() {
    // 2t² to the middle: 0 at 0, 2/16 = 0.125 at a quarter, ½ at ½; 1 − (2 − 2t)²/2 after:
    // 1 − ¼/2 = 0.875 at three quarters, 1 at 1.
    assert_eq!(ease(0.0), 0.0);
    assert_eq!(ease(0.25), 0.125);
    assert_eq!(ease(0.5), 0.5);
    assert_eq!(ease(0.75), 0.875);
    assert_eq!(ease(1.0), 1.0);
}

#[test]
fn a_turn_goes_the_short_way_round() {
    // From 350° to 10°: 20° on, past 360, not 340° back; from 10° to 350°, 20° back. Half a
    // turn either way is −π, as the prototype brings it into [−π, π).
    near1(
        short_turn(350f64.to_radians(), 10f64.to_radians()),
        20f64.to_radians(),
        1e-12,
    );
    near1(
        short_turn(10f64.to_radians(), 350f64.to_radians()),
        -20f64.to_radians(),
        1e-12,
    );
    near1(short_turn(0.0, PI), -PI, 1e-15);
    near1(short_turn(0.0, 0.0), 0.0, 0.0);
}

#[test]
fn from_350_to_10_degrees_the_view_passes_the_front_not_the_back() {
    // Halfway, the azimuth is 0: the eye at (0, −100, 0). A quarter of the way, 355°: the
    // eye at (100 sin 355°, −100 cos 355°, 0) = (−8.7156, −99.6195, 0). The long way round
    // would pass the back, the eye at (0, 100, 0).
    let (from, to) = (level_at(350.0), level_at(10.0));
    let half = between(&from, &to, 0.5).unwrap();
    near(half.eye, [0.0, -100.0, 0.0], 1e-12);
    near(half.up, [0.0, 0.0, 1.0], 1e-15);
    let quarter = between(&from, &to, 0.25).unwrap();
    near(
        quarter.eye,
        [-8.715_574_274_765_8, -99.619_469_809_174_6, 0.0],
        1e-12,
    );
}

#[test]
fn between_two_views_each_part_moves_as_the_prototypes_does() {
    // From the front, 80 high, onto the origin, to the right (azimuth 90°), 40 high, onto
    // (10, 0, 20), 100 out. Halfway: azimuth 45°, level, the target (5, 0, 10), the height
    // 60, the eye 100 out along (sin 45°, −cos 45°, 0), Z up.
    let right = camera(40.0, [10.0, 0.0, 20.0], [100.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let half = between(&front(), &right, 0.5).unwrap();
    let r = 100.0 * FRAC_1_SQRT_2;
    near(half.target, [5.0, 0.0, 10.0], 1e-15);
    near(half.eye, [5.0 + r, -r, 10.0], 1e-12);
    near(half.up, [0.0, 0.0, 1.0], 1e-15);
    assert_eq!(height(&half), 60.0);
    // At the ends, each end, to within its rounding.
    let start = between(&front(), &right, 0.0).unwrap();
    near(start.eye, front().eye, 1e-12);
    let end = between(&front(), &right, 1.0).unwrap();
    near(end.eye, right.eye, 1e-12);
    near(end.target, right.target, 0.0);
    assert_eq!(height(&end), 40.0);
}

#[test]
fn into_the_top_view_the_elevation_rises_and_the_up_turns_into_the_tops() {
    // To the top, Y up: straight above, its azimuth read from its right (+x), 0. Halfway
    // from the front: elevation 45°, azimuth 0, so the eye at 100 (0, −cos 45°, sin 45°) and
    // the up the prototype's, (−sin 45° sin 0, sin 45° cos 0, cos 45°) = (0, ½√2, ½√2).
    let top = camera(80.0, [0.0; 3], [0.0, 0.0, 100.0], [0.0, 1.0, 0.0]);
    let r = FRAC_1_SQRT_2;
    let half = between(&front(), &top, 0.5).unwrap();
    near(half.eye, [0.0, -100.0 * r, 100.0 * r], 1e-12);
    near(half.up, [0.0, r, r], 1e-15);
    let end = between(&front(), &top, 1.0).unwrap();
    near(end.eye, [0.0, 0.0, 100.0], 1e-12);
    near(end.up, [0.0, 1.0, 0.0], 1e-15);
    // A top view with −y up has its right −x: azimuth π, which is −π from the front the
    // short way. Halfway: azimuth −90°, elevation 45°: the eye along (−cos 45°, 0, sin 45°),
    // up (−sin 45° sin(−90°), sin 45° cos(−90°), cos 45°) = (½√2, 0, ½√2).
    let flipped = camera(80.0, [0.0; 3], [0.0, 0.0, 100.0], [0.0, -1.0, 0.0]);
    let half = between(&front(), &flipped, 0.5).unwrap();
    near(half.eye, [-100.0 * r, 0.0, 100.0 * r], 1e-12);
    near(half.up, [r, 0.0, r], 1e-15);
    let end = between(&front(), &flipped, 1.0).unwrap();
    near(end.up, [0.0, -1.0, 0.0], 1e-15);
}

#[test]
fn down_into_the_bottom_view_the_up_never_flips() {
    // The app's bottom, Y up, looking up from under: its right, up × back = (0, 1, 0) ×
    // (0, 0, −1) = (−1, 0, 0), azimuth π. In a hundred steps from the front the up turns
    // by at most a few degrees a step (cos > 0.99), never through a flip, and ends Y up.
    let bottom = camera(80.0, [0.0; 3], [0.0, 0.0, -100.0], [0.0, 1.0, 0.0]);
    let mut last = front().up;
    for k in 1..=100 {
        let step = between(&front(), &bottom, f64::from(k) / 100.0).unwrap();
        let frame = step.frame().unwrap();
        let turned = silico_protocol::vec3::dot(frame.up, last);
        assert!(turned > 0.99, "step {k}: {:?} after {last:?}", frame.up);
        last = frame.up;
    }
    near(last, [0.0, 1.0, 0.0], 1e-15);
}

#[test]
fn a_view_turned_about_its_own_axis_rolls_the_short_way() {
    // The front with +x up (a sketch's Look At whose y is the world's x): the up's roll from
    // Z, the prototype's up, is atan2(−x·(1, 0, 0), x·(0, 0, 1)) = −90°. Halfway, −45°: the up
    // Z cos 45° + X sin 45° = (½√2, 0, ½√2), the eye where it was.
    let rolled = camera(80.0, [0.0; 3], [0.0, -100.0, 0.0], [1.0, 0.0, 0.0]);
    let half = between(&front(), &rolled, 0.5).unwrap();
    near(half.eye, [0.0, -100.0, 0.0], 1e-12);
    near(half.up, [FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2], 1e-15);
    // Upside down (−Z up), a half turn: −π, as the short way gives it, so through +x up.
    let upside_down = camera(80.0, [0.0; 3], [0.0, -100.0, 0.0], [0.0, 0.0, -1.0]);
    let half = between(&front(), &upside_down, 0.5).unwrap();
    near(half.up, [1.0, 0.0, 0.0], 1e-15);
}

#[test]
fn a_perspective_view_moves_its_eyes_distance_straight() {
    // 30° perspective from 100 out to 300 out along −y: halfway 200 out, the angle kept.
    let at = |distance: f64| Camera {
        projection: Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: [0.0, -distance, 0.0],
        target: [0.0; 3],
        up: [0.0, 0.0, 1.0],
    };
    let half = between(&at(100.0), &at(300.0), 0.5).unwrap();
    near(half.eye, [0.0, -200.0, 0.0], 1e-12);
    assert_eq!(
        half.projection,
        Projection::Perspective {
            field_of_view: 30.0
        }
    );
}

#[test]
fn no_transition_joins_two_projections_or_two_angles_or_a_camera_with_no_frame() {
    let perspective = |field_of_view: f64| Camera {
        projection: Projection::Perspective { field_of_view },
        ..front()
    };
    assert_eq!(between(&front(), &perspective(30.0), 0.5), None);
    assert_eq!(between(&perspective(30.0), &front(), 0.5), None);
    assert_eq!(between(&perspective(30.0), &perspective(40.0), 0.5), None);
    let no_frame = Camera {
        eye: [0.0; 3],
        ..front()
    };
    assert_eq!(between(&front(), &no_frame, 0.5), None);
    assert_eq!(between(&no_frame, &front(), 0.5), None);
}

#[test]
fn a_transition_eases_over_its_time_and_ends_on_its_view_exactly() {
    // Begun at 10 s for a quarter of a second (each moment below a binary fraction, so its
    // time gone is exact), from the front to the right (azimuth 90°): at 10.0625, a quarter
    // of its time, the ease is 0.125, so azimuth 11.25°; at 10.125, half, 45°; from 10.25
    // on, the right view to the bit, however its parts round.
    let right = Camera {
        projection: Projection::Orthographic {
            view_height: 41.123_456_789_012_3,
        },
        eye: [100.000_000_000_000_1, 1e-17, 0.3],
        target: [0.1, 1e-17, 0.3],
        up: [0.0, 0.0, 1.0],
    };
    let motion = Motion {
        now: 10.0,
        seconds: 0.25,
    };
    let transition = Transition::new(front(), right.clone(), front(), motion);
    let azimuth = |step: Step| {
        let Step::Moving(camera) = step else {
            panic!("ended: {step:?}");
        };
        let back = camera.frame().unwrap().back;
        libm::atan2(back[0], -back[1]).to_degrees()
    };
    near1(azimuth(transition.at(10.0)), 0.0, 1e-12);
    near1(azimuth(transition.at(10.0625)), 11.25, 1e-9);
    near1(azimuth(transition.at(10.125)), 45.0, 1e-9);
    assert_eq!(transition.at(10.25), Step::Ended(right.clone()));
    assert_eq!(transition.at(11.0), Step::Ended(right.clone()));
    assert_eq!(transition.at(f64::NAN), Step::Ended(right.clone()));
    // A jump ends at once.
    let jump = Transition::new(front(), right.clone(), front(), Motion::JUMP);
    assert_eq!(jump.at(0.0), Step::Ended(right));
}

#[test]
fn roll_turns_the_short_way_across_a_half_turn() {
    // Rolled 170° and −170° from Z up: 20° apart the short way, through −Z up halfway; 340°
    // apart the long way, through +Z.
    let r = 170f64.to_radians();
    let a = camera(
        80.0,
        [0.0; 3],
        [0.0, -100.0, 0.0],
        [-libm::sin(r), 0.0, libm::cos(r)],
    );
    let b = camera(
        80.0,
        [0.0; 3],
        [0.0, -100.0, 0.0],
        [libm::sin(r), 0.0, libm::cos(r)],
    );
    near(between(&a, &b, 0.5).unwrap().up, [0.0, 0.0, -1.0], 1e-12);
}

/// How far the image turns about the view's axis, in degrees, over a hundred steps from
/// `from` to `to`: each step's up's turn from the step before's, about the step's view axis.
fn twist(from: &Camera, to: &Camera) -> f64 {
    use silico_protocol::vec3::{cross, dot};
    let mut last = from.frame().unwrap().up;
    let mut turned = 0.0;
    for k in 1..=100 {
        let frame = between(from, to, f64::from(k) / 100.0)
            .unwrap()
            .frame()
            .unwrap();
        turned += libm::atan2(dot(cross(last, frame.up), frame.back), dot(last, frame.up));
        last = frame.up;
    }
    turned.to_degrees()
}

#[test]
fn from_just_over_the_top_to_the_top_the_image_hardly_turns() {
    // The front orbited over the top, by a drag of 182, 190, and 200 points down (91°, 95°,
    // 100°: `navigate::orbit` turns the model with the pointer, so the eye goes up), and 3
    // points across too (1.5° about Z): upside down from behind, so its
    // azimuth and its roll each a half turn from the top's (Y up, azimuth 0). Each taken the
    // short way, −π and −π, their image's turns add to a whole turn; taken together, the
    // roll turns +π and they cancel, so to the top the image turns a few degrees at most
    // (the drag across's 1.5°, and what tipping back up to 10° over the top adds: 2.4° at
    // most here), not a whole turn. As made by hand with the azimuth +π and −π (its x +0
    // and −0) at 91°, the same.
    let top = camera(80.0, [0.0; 3], [0.0, 0.0, 100.0], [0.0, 1.0, 0.0]);
    let mut starts = Vec::new();
    for up in [182.0, 190.0, 200.0] {
        for across in [0.0, 3.0] {
            starts.push(crate::navigate::orbit(&front(), across, up).unwrap());
        }
    }
    let a = 91f64.to_radians();
    for x in [0.0, -0.0] {
        starts.push(camera(
            80.0,
            [0.0; 3],
            [x, -100.0 * libm::cos(a), 100.0 * libm::sin(a)],
            [x, libm::sin(a), libm::cos(a)],
        ));
    }
    for start in &starts {
        let turned = twist(start, &top);
        assert!(turned.abs() < 3.0, "{turned}° from {start:?}");
    }
}
