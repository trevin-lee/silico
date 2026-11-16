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

/// The end the pointer at `at` picks of `curve`, which the core takes as a line or an arc.
fn end_at(s: &Sketch, curve: &str, at: Xy) -> Option<End> {
    extend_end(s, &id(curve), at).unwrap()
}

/// `s` with an arc `name` about `centre`, radius `r`, counter-clockwise from `from`° to
/// `to`°: its centre `c` and ends `s` and `e` in place of its ID's first character.
fn with_arc(mut s: Sketch, name: &str, centre: Xy, r: f64, from: f64, to: f64) -> Sketch {
    let at = |deg: f64| {
        let a = deg.to_radians();
        [centre[0] + r * libm::cos(a), centre[1] + r * libm::sin(a)]
    };
    let rest = &name[1..];
    let (c, st, en) = (format!("c{rest}"), format!("s{rest}"), format!("e{rest}"));
    let (start, end) = (at(from), at(to));
    s.points.push(point(&c, centre[0], centre[1]));
    s.points.push(point(&st, start[0], start[1]));
    s.points.push(point(&en, end[0], end[1]));
    s.arcs.push(arc(name, &c, &st, &en));
    s
}

/// The place `degrees` round from +x at `r` from `centre`.
fn round_at(centre: Xy, r: f64, degrees: f64) -> Xy {
    let a = degrees.to_radians();
    [centre[0] + r * libm::cos(a), centre[1] + r * libm::sin(a)]
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
    assert_eq!(end_at(&s, "na000001", [8.0, 0.2]), Some(End::End));
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
    assert_eq!(end_at(&s, "na000001", [2.0, -0.2]), Some(End::Start));
    let refused = extend_preview(&s, &line, End::Start).unwrap_err();
    assert_eq!(
        refused.message(),
        "line na000001 meets no other curve past its start"
    );
}

#[test]
fn a_lines_halves_meet_at_its_midpoint_which_picks_its_end() {
    let s = line_and_wall();
    // The pointer put on the line: (5, 0) is the midpoint, t = 0.5, the end's; a hair
    // before it the start's. Off the line (the foot's what counts) and past either end
    // alike.
    assert_eq!(end_at(&s, "na000001", [5.0, 0.0]), Some(End::End));
    assert_eq!(end_at(&s, "na000001", [4.999, 0.3]), Some(End::Start));
    assert_eq!(end_at(&s, "na000001", [10.4, 0.0]), Some(End::End));
    assert_eq!(end_at(&s, "na000001", [-0.4, 0.0]), Some(End::Start));
}

#[test]
fn a_hover_near_an_arcs_end_extends_it_counter_clockwise_to_the_ledge() {
    let s = arc_and_ledge();
    let arc_id = id("aa000001");
    // At 80°, past the mid-angle 45°: the end, at 90°, goes on counter-clockwise to y = 5's
    // crossing at 150°, (−√75, 5), 60° on. Drawn counter-clockwise from the end to there.
    assert_eq!(end_at(&s, "aa000001", on_ten(80.0)), Some(End::End));
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
    assert_eq!(end_at(&s, "aa000001", on_ten(10.0)), Some(End::Start));
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
    // The mid-angle, 45° (along (1, 1), which `atan2` gives as π/4 exactly, half of π/2),
    // is the end's; 44° the start's.
    assert_eq!(end_at(&s, "aa000001", [5.0, 5.0]), Some(End::End));
    assert_eq!(end_at(&s, "aa000001", on_ten(44.0)), Some(End::Start));
    // Past the sweep: 100° is 10° past the end (and 260° short of the start); −10° is 10°
    // short of the start. 225° (along (−1, −1), −3π/4 exactly) is 135° from either: the
    // start's.
    assert_eq!(end_at(&s, "aa000001", on_ten(100.0)), Some(End::End));
    assert_eq!(end_at(&s, "aa000001", on_ten(-10.0)), Some(End::Start));
    assert_eq!(end_at(&s, "aa000001", [-5.0, -5.0]), Some(End::Start));
    // Off the rim the angle's what counts; at the centre there's no half.
    assert_eq!(end_at(&s, "aa000001", [3.0, 0.5]), Some(End::Start));
    assert_eq!(end_at(&s, "aa000001", [0.0, 0.0]), None);
}

#[test]
fn a_circle_a_point_a_constraint_or_no_member_is_refused_in_the_commands_words() {
    let s = constrained(
        arc_and_ledge(),
        vec![Form::Horizontal {
            line: el("na000001"),
        }],
    );
    // A circle has no end: no half, and its preview refused, each as `sketch_extend`
    // refuses it.
    let circle = "circle ca000001 can't be extended: only a line or an arc can";
    let refused = extend_end(&s, &id("ca000001"), [52.0, 50.0]).unwrap_err();
    assert_eq!(refused.message(), circle);
    let refused = extend_preview(&s, &id("ca000001"), End::End).unwrap_err();
    assert_eq!(refused.message(), circle);
    // A point, a constraint, and an ID the sketch doesn't have (an axis is none of its
    // members) likewise.
    let refused = |id_: &str, at: Xy| extend_end(&s, &id(id_), at).unwrap_err();
    assert_eq!(
        refused("pb000002", [-1.0, 5.0]).message(),
        "point pb000002 can't be extended: only a line or an arc can"
    );
    assert_eq!(
        refused("k0000001", [-10.0, 5.0]).message(),
        "constraint k0000001 can't be extended: only a line or an arc can"
    );
    assert_eq!(
        refused("zz000001", [0.0, 5.0]).message(),
        "the sketch has no member zz000001"
    );
    // A pointer that isn't a place is on neither half.
    assert_eq!(end_at(&s, "na000001", [f64::NAN, 5.0]), None);
}

#[test]
fn a_line_of_no_length_or_an_arc_of_no_sweep_is_refused_for_what_it_lacks() {
    // A line from (5, 5) to (5, 5); an arc about (0, 0) from (10, 0) to (20, 0), its ends
    // both at 0° from its centre.
    let mut s = empty();
    s.points = vec![
        point("pa000001", 5.0, 5.0),
        point("pa000002", 5.0, 5.0),
        point("pb000001", 0.0, 0.0),
        point("pb000002", 10.0, 0.0),
        point("pb000003", 20.0, 0.0),
    ];
    s.lines = vec![line("na000001", "pa000001", "pa000002")];
    s.arcs = vec![arc("aa000001", "pb000001", "pb000002", "pb000003")];
    let no_length = "line na000001 has no length, so it has no direction to extend";
    assert_eq!(
        extend_end(&s, &id("na000001"), [5.0, 5.0])
            .unwrap_err()
            .message(),
        no_length
    );
    // `next_crossing`, which `sketch_extend` with `next_curve` extends by, says the same of
    // it, either end: the hover and the command agree. (The gate refuses such a line, and
    // the solve a result with one, so a session never reaches the command with it.)
    for end in [End::Start, End::End] {
        assert_eq!(
            crate::extend::next_crossing(&s, &id("na000001"), end)
                .unwrap_err()
                .message(),
            no_length
        );
    }
    assert_eq!(
        extend_end(&s, &id("aa000001"), [15.0, 0.0])
            .unwrap_err()
            .message(),
        "arc aa000001's start and end are at one angle from its centre, so it has no circle to extend around"
    );
}

#[test]
fn halves_are_picked_off_the_origin_and_off_the_axes() {
    // A line from (20, 0) to (30, 0): its midpoint (25, 0); (22, 0) is 0.2 along, the
    // start's half.
    let s = with_line(empty(), "na000001", [20.0, 0.0], [30.0, 0.0]);
    assert_eq!(end_at(&s, "na000001", [22.0, 0.0]), Some(End::Start));
    assert_eq!(end_at(&s, "na000001", [28.0, 0.0]), Some(End::End));
    // A vertical wall from (30, −8) to (30, 8): its midpoint (30, 0); (30, −5) is
    // 3/16 along, the start's, and (30, 5) 13/16, the end's.
    let s = with_line(empty(), "nb000001", [30.0, -8.0], [30.0, 8.0]);
    assert_eq!(end_at(&s, "nb000001", [30.0, -5.0]), Some(End::Start));
    assert_eq!(end_at(&s, "nb000001", [30.0, 5.0]), Some(End::End));
    // An arc about (5, 5), radius 10, from 150° to 240°, across ±π: `atan2` gives its
    // start 150° and its end −120°, taken a turn on to 240°; its sweep 90°, its mid-angle
    // 195°. At 190° (`atan2` −170°: −170° − 150° + 360° = 40° into it, under 45°) the
    // start's; at 200° (−160°: 50° in) the end's; at 170° (20° in) the start's, and at
    // 230° (80° in) the end's.
    let centre = [5.0, 5.0];
    let s = with_arc(empty(), "aa000001", centre, 10.0, 150.0, 240.0);
    for (degrees, end) in [
        (190.0, End::Start),
        (200.0, End::End),
        (170.0, End::Start),
        (230.0, End::End),
    ] {
        assert_eq!(
            end_at(&s, "aa000001", round_at(centre, 10.0, degrees)),
            Some(end),
            "{degrees}°"
        );
    }
}

#[test]
fn past_an_arcs_sweep_the_nearer_end_wins_either_side_of_halfway_round() {
    // The quarter from 0° to 90°: past it, the ends are equally near at 225°, 135° past the
    // end and 135° short of the start. 220° is 130° past the end and 140° short of the
    // start: the end's; 230°, 140° and 130°: the start's.
    let s = arc_and_ledge();
    assert_eq!(end_at(&s, "aa000001", on_ten(220.0)), Some(End::End));
    assert_eq!(end_at(&s, "aa000001", on_ten(230.0)), Some(End::Start));
    // An arc from 0° to 350°: its gap is 10°, halved at 355°. 352° is 2° past the end and
    // 8° short of the start: the end's; 358°, 8° and 2°: the start's.
    let s = with_arc(empty(), "aa000002", [0.0, 0.0], 10.0, 0.0, 350.0);
    assert_eq!(end_at(&s, "aa000002", on_ten(352.0)), Some(End::End));
    assert_eq!(end_at(&s, "aa000002", on_ten(358.0)), Some(End::Start));
}

#[test]
fn a_lines_start_extended_is_drawn_from_its_start_to_the_wall_behind_it() {
    // `line_and_wall` with a second wall at x = −20, from (−20, −10) to (−20, 10): near
    // (2, 0), the start's half, its start (0, 0) goes on to (−20, 0).
    let s = with_line(line_and_wall(), "nc000001", [-20.0, -10.0], [-20.0, 10.0]);
    assert_eq!(end_at(&s, "na000001", [2.0, 0.0]), Some(End::Start));
    let preview = extend_preview(&s, &id("na000001"), End::Start).unwrap();
    assert_eq!(preview.lines.len(), 1, "{preview:?}");
    assert!(close(preview.lines[0][0], [0.0, 0.0]), "{preview:?}");
    assert!(close(preview.lines[0][1], [-20.0, 0.0]), "{preview:?}");
}

#[test]
fn a_construction_curves_extension_is_previewed_as_construction() {
    let mut s = line_and_wall();
    s.lines[0].construction = true;
    let preview = extend_preview(&s, &id("na000001"), End::End).unwrap();
    assert!(preview.construction);
}

#[test]
fn a_construction_arcs_extension_is_previewed_as_construction() {
    let mut s = arc_and_ledge();
    s.arcs[0].construction = true;
    let preview = extend_preview(&s, &id("aa000001"), End::End).unwrap();
    assert_eq!(preview.arcs.len(), 1);
    assert!(preview.construction);
}
