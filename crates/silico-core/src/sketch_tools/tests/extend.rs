//! Extend Entities' hover: the end the half under the pointer picks, and the extension a
//! click would make, to the next curve. Every place worked by hand in the comments.

use super::*;
use crate::extend::End;

/// A line na000001 from (0, 0) to (10, 0), and a line nb000001 along x = 30 from
/// (30, −10) to (30, 10).
fn line_and_wall() -> Sketch {
    let mut s = empty();
    s.points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", 10.0, 0.0),
        point("pb000001", 30.0, -10.0),
        point("pb000002", 30.0, 10.0),
    ];
    s.lines = vec![
        line("na000001", "pa000001", "pa000002"),
        line("nb000001", "pb000001", "pb000002"),
    ];
    s
}

/// An arc aa000001 about (0, 0), radius 10, counter-clockwise from (10, 0), 0°, to (0, 10),
/// 90°; a line na000001 along y = 5 from (−20, 5) to (−1, 5); and a circle ca000001 of
/// radius 2 about (50, 50), out of everything's way.
fn arc_and_ledge() -> Sketch {
    let mut s = empty();
    s.points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", 10.0, 0.0),
        point("pa000003", 0.0, 10.0),
        point("pb000001", -20.0, 5.0),
        point("pb000002", -1.0, 5.0),
        point("pc000001", 50.0, 50.0),
    ];
    s.arcs = vec![arc("aa000001", "pa000001", "pa000002", "pa000003")];
    s.lines = vec![line("na000001", "pb000001", "pb000002")];
    s.circles = vec![circle("ca000001", "pc000001", 2.0)];
    s
}

/// (x, y) at `degrees` on the circle of radius 10 about the origin.
fn on_ten(degrees: f64) -> Xy {
    let a = degrees.to_radians();
    [10.0 * libm::cos(a), 10.0 * libm::sin(a)]
}

fn close(a: Xy, b: Xy) -> bool {
    (a[0] - b[0]).abs() <= 1e-9 && (a[1] - b[1]).abs() <= 1e-9
}

/// Where y = 5 meets x² + y² = 100 on the left: x = −√75, at 150°.
fn ledge_crossing() -> Xy {
    [-(75.0_f64.sqrt()), 5.0]
}

#[test]
fn a_hover_near_a_lines_end_previews_it_extended_to_the_wall_and_near_its_start_nothing() {
    let s = line_and_wall();
    let line = id("na000001");
    // Near (8, 0): 0.8 of the way along, past the midpoint (5, 0), so its end, (10, 0),
    // goes on along y = 0 to x = 30, the wall.
    let end = extend_end(&s, &line, [8.0, 0.2]);
    assert_eq!(end, Some(End::End));
    let preview = extend_preview(&s, &line, End::End).unwrap();
    assert_eq!(preview.lines.len(), 1);
    assert!(close(preview.lines[0][0], [10.0, 0.0]), "{preview:?}");
    assert!(close(preview.lines[0][1], [30.0, 0.0]), "{preview:?}");
    assert_eq!(preview.points.len(), 1);
    assert!(close(preview.points[0], [30.0, 0.0]));
    assert!(preview.arcs.is_empty() && preview.circles.is_empty());
    assert!(!preview.construction);
    // Near (2, 0): 0.2 of the way, before the midpoint, so its start, (0, 0); to its left
    // along y = 0 there's nothing, so no preview, and the core's own words for it.
    assert_eq!(extend_end(&s, &line, [2.0, -0.2]), Some(End::Start));
    let refused = extend_preview(&s, &line, End::Start).unwrap_err();
    assert_eq!(
        refused.message(),
        "line na000001 meets no other curve past its start"
    );
}

#[test]
fn a_lines_halves_meet_at_its_midpoint_which_picks_its_end() {
    let s = line_and_wall();
    let line = id("na000001");
    // The pointer put on the line: (5, 0) is the midpoint, t = 0.5, the end's; a hair
    // before it the start's. Off the line (the foot's what counts) and past either end
    // alike.
    assert_eq!(extend_end(&s, &line, [5.0, 0.0]), Some(End::End));
    assert_eq!(extend_end(&s, &line, [4.999, 0.3]), Some(End::Start));
    assert_eq!(extend_end(&s, &line, [10.4, 0.0]), Some(End::End));
    assert_eq!(extend_end(&s, &line, [-0.4, 0.0]), Some(End::Start));
}

#[test]
fn a_hover_near_an_arcs_end_extends_it_counter_clockwise_to_the_ledge() {
    let s = arc_and_ledge();
    let arc_id = id("aa000001");
    // At 80°, past the mid-angle 45°: the end, at 90°, goes on counter-clockwise to y = 5's
    // crossing at 150°, (−√75, 5), 60° on. Drawn counter-clockwise from the end to there.
    assert_eq!(extend_end(&s, &arc_id, on_ten(80.0)), Some(End::End));
    let preview = extend_preview(&s, &arc_id, End::End).unwrap();
    assert_eq!(preview.arcs.len(), 1);
    let drawn = preview.arcs[0];
    assert!(close(drawn.center, [0.0, 0.0]));
    assert!(close(drawn.start, [0.0, 10.0]), "{drawn:?}");
    assert!(close(drawn.end, ledge_crossing()), "{drawn:?}");
    assert!(close(drawn.end, [-8.660_254_037_844_386, 5.0]));
    assert!(close(preview.points[0], ledge_crossing()));
    assert!(preview.lines.is_empty());
}

#[test]
fn a_hover_near_an_arcs_start_extends_it_clockwise_two_hundred_and_ten_degrees_to_the_ledge() {
    let s = arc_and_ledge();
    let arc_id = id("aa000001");
    // At 10°, before the mid-angle: the start, at 0°, goes on clockwise, down through 270°
    // and 180°. The first thing it meets is y = 5 at 150°, 210° clockwise from 0° (y = 5's
    // other crossing, x = +√75 at 30°, is off its piece, which ends at x = −1).
    // `next_crossing` lets an arc's end go round short of its other end: less than a turn
    // less the sweep, 360° − 90° = 270°, and 210° is less, so it gets there.
    // Drawn counter-clockwise from the place, 150°, on round to the start, 360°.
    assert_eq!(extend_end(&s, &arc_id, on_ten(10.0)), Some(End::Start));
    let preview = extend_preview(&s, &arc_id, End::Start).unwrap();
    assert_eq!(preview.arcs.len(), 1);
    let drawn = preview.arcs[0];
    assert!(close(drawn.center, [0.0, 0.0]));
    assert!(close(drawn.start, ledge_crossing()), "{drawn:?}");
    assert!(close(drawn.end, [10.0, 0.0]), "{drawn:?}");
}

#[test]
fn an_arcs_halves_meet_at_its_mid_angle_and_past_its_sweep_the_nearer_end_wins() {
    let s = arc_and_ledge();
    let arc_id = id("aa000001");
    // The mid-angle, 45° (along (1, 1), which `atan2` gives as π/4 exactly, half of π/2),
    // is the end's; 44° the start's.
    assert_eq!(extend_end(&s, &arc_id, [5.0, 5.0]), Some(End::End));
    assert_eq!(extend_end(&s, &arc_id, on_ten(44.0)), Some(End::Start));
    // Past the sweep: 100° is 10° past the end (and 260° short of the start); −10° is 10°
    // short of the start. 225° (along (−1, −1), −3π/4 exactly) is 135° from either: the
    // start's.
    assert_eq!(extend_end(&s, &arc_id, on_ten(100.0)), Some(End::End));
    assert_eq!(extend_end(&s, &arc_id, on_ten(-10.0)), Some(End::Start));
    assert_eq!(extend_end(&s, &arc_id, [-5.0, -5.0]), Some(End::Start));
    // Off the rim the angle's what counts; at the centre there's no half.
    assert_eq!(extend_end(&s, &arc_id, [3.0, 0.5]), Some(End::Start));
    assert_eq!(extend_end(&s, &arc_id, [0.0, 0.0]), None);
}

#[test]
fn a_circle_a_point_a_constraint_or_no_member_takes_nothing() {
    let s = constrained(
        arc_and_ledge(),
        vec![Form::Horizontal {
            line: el("na000001"),
        }],
    );
    // A circle has no end: no half, and the core refuses its preview as it refuses its
    // extend.
    assert_eq!(extend_end(&s, &id("ca000001"), [52.0, 50.0]), None);
    let refused = extend_preview(&s, &id("ca000001"), End::End).unwrap_err();
    assert_eq!(
        refused.message(),
        "circle ca000001 can't be extended: only a line or an arc can"
    );
    // A point, a constraint, and an ID the sketch doesn't have (an axis is none of its
    // members) take nothing.
    assert_eq!(extend_end(&s, &id("pb000002"), [-1.0, 5.0]), None);
    assert_eq!(extend_end(&s, &id("k0000001"), [-10.0, 5.0]), None);
    assert_eq!(extend_end(&s, &id("zz000001"), [0.0, 5.0]), None);
    // And a pointer that isn't a place.
    assert_eq!(extend_end(&s, &id("na000001"), [f64::NAN, 5.0]), None);
}

#[test]
fn a_construction_curves_extension_is_previewed_as_construction() {
    let mut s = line_and_wall();
    s.lines[0].construction = true;
    let preview = extend_preview(&s, &id("na000001"), End::End).unwrap();
    assert!(preview.construction);
}
