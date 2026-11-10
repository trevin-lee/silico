//! The ground grid, worked by hand: its strokes as the prototype's `buildGrid` makes them, and
//! where they land.

use eframe::egui::epaint::{ColorMode, PathShape};
use eframe::egui::{Color32, Pos2, Rect, Shape, pos2, vec2};
use silico_protocol::{Camera, Projection};

use super::{GridStroke, SEGMENT_MM, fade, shapes, strokes};

/// The stroke from `a` to `b` on the plane.
fn stroke_from(strokes: &[GridStroke], a: [f64; 2], b: [f64; 2]) -> GridStroke {
    *strokes
        .iter()
        .find(|s| s.ends == [[a[0], a[1], 0.0], [b[0], b[1], 0.0]])
        .unwrap_or_else(|| panic!("no stroke from {a:?} to {b:?}"))
}

const MINOR: [u8; 3] = [0x3a, 0x39, 0x39];
const MAJOR: [u8; 3] = [0x64, 0x63, 0x63];

#[test]
fn the_fade_is_whole_at_the_origin_half_at_100_mm_and_none_from_200_mm_on() {
    assert_eq!(fade(0.0), 1.0);
    assert_eq!(fade(50.0), 0.75);
    assert_eq!(fade(100.0), 0.5);
    assert_eq!(fade(200.0), 0.0);
    assert_eq!(fade(300.0), 0.0);
}

#[test]
fn the_grids_strokes_are_the_prototypes_worked_by_hand() {
    let all = strokes();
    // 40 places (every 10 mm from −200 to 200 but 0), two lines at each, each in 20 segments
    // of 20 mm; then the three axes.
    assert_eq!(all.len(), 40 * 2 * 20 + 3);
    let (lines, axes) = all.split_at(1600);
    // The prototype's order: at x = −200, from (−200, −200), the line along y, then the one
    // along x; segment by segment; then the next place, −190, from index 40.
    assert_eq!(
        lines[0].ends,
        [[-200.0, -200.0, 0.0], [-200.0, -180.0, 0.0]]
    );
    assert_eq!(
        lines[1].ends,
        [[-200.0, -200.0, 0.0], [-180.0, -200.0, 0.0]]
    );
    assert_eq!(
        lines[2].ends,
        [[-200.0, -180.0, 0.0], [-200.0, -160.0, 0.0]]
    );
    assert_eq!(
        lines[40].ends,
        [[-190.0, -200.0, 0.0], [-190.0, -180.0, 0.0]]
    );
    assert_eq!(lines[1599].ends, [[180.0, 200.0, 0.0], [200.0, 200.0, 0.0]]);
    // None on x = 0 or y = 0 (the axes are there); all on z = 0, inside ±200, 20 mm long, a
    // point wide.
    for line in lines {
        let [[x0, y0, z0], [x1, y1, z1]] = line.ends;
        assert!(
            !(x0 == 0.0 && x1 == 0.0) && !(y0 == 0.0 && y1 == 0.0),
            "{line:?}"
        );
        assert_eq!((z0, z1), (0.0, 0.0));
        assert!(
            [x0, y0, x1, y1].iter().all(|c| c.abs() <= 200.0),
            "{line:?}"
        );
        assert_eq!((x1 - x0).abs() + (y1 - y0).abs(), SEGMENT_MM);
        assert_eq!(line.width, 1.0);
    }
    // Major every 50 mm, Gray.400 (#646363), the rest minor (#3a3939): the 8 places ±50,
    // ±100, ±150, ±200, 40 strokes each.
    assert_eq!(lines.iter().filter(|l| l.rgb == MAJOR).count(), 8 * 40);
    assert_eq!(lines.iter().filter(|l| l.rgb == MINOR).count(), 32 * 40);
    // The fade at each end, by its distance from the origin: (60, 80) is 100 mm out, so half
    // of the minor lines' 191 (75%), 95.5; (60, 100) is √13600 = 116.619 mm out, so
    // 191 (1 − 0.583095) = 79.6288.
    let at_100 = stroke_from(lines, [60.0, 80.0], [60.0, 100.0]);
    assert_eq!(at_100.rgb, MINOR);
    assert_eq!(at_100.alpha[0], 95.5);
    assert!((at_100.alpha[1] - 79.628_818_8).abs() < 1e-6, "{at_100:?}");
    // (120, 160) is 200 mm out: none left.
    assert_eq!(
        stroke_from(lines, [120.0, 160.0], [120.0, 180.0]).alpha[0],
        0.0
    );
    // Near the origin: (10, 0), 10 mm out, 191 · 0.95 = 181.45; the major line x = 50 at
    // (50, 0), 230 (90%) · 0.75 = 172.5; the line y = 10 at (0, 10) as x = 10 at (10, 0).
    let ten = stroke_from(lines, [10.0, 0.0], [10.0, 20.0]);
    assert!((ten.alpha[0] - 181.45).abs() < 1e-9, "{ten:?}");
    let fifty = stroke_from(lines, [50.0, 0.0], [50.0, 20.0]);
    assert_eq!((fifty.rgb, fifty.alpha[0]), (MAJOR, 172.5));
    assert_eq!(
        stroke_from(lines, [0.0, 10.0], [20.0, 10.0]).alpha[0],
        ten.alpha[0]
    );
    // A corner's segment, both ends past 200 mm: wholly transparent.
    assert_eq!(lines[0].alpha, [0.0, 0.0]);
    // The axes, unfaded at 230 (90%), 1.5 points wide: X (#ff6981) across ±200, Y (#00b266)
    // across ±200, Z (#7a95ff) up 40 from the origin.
    let axis = |ends, rgb| GridStroke {
        ends,
        rgb,
        alpha: [230.0; 2],
        width: 1.5,
    };
    assert_eq!(
        axes,
        [
            axis([[-200.0, 0.0, 0.0], [200.0, 0.0, 0.0]], [0xff, 0x69, 0x81]),
            axis([[0.0, -200.0, 0.0], [0.0, 200.0, 0.0]], [0x00, 0xb2, 0x66]),
            axis([[0.0; 3], [0.0, 0.0, 40.0]], [0x7a, 0x95, 0xff]),
        ]
    );
}

/// From above, orthographic, 400 mm high on 400 by 400 pixels at a pixel a point: (x, y)
/// lands at pixel (200 + x, 200 − y), here from [`rect`]'s corner, (10, 20).
fn from_above() -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 400.0 },
        eye: [0.0, 0.0, 100.0],
        target: [0.0; 3],
        up: [0.0, 1.0, 0.0],
    }
}

const SIZE: [u32; 2] = [400, 400];

fn rect() -> Rect {
    Rect::from_min_size(pos2(10.0, 20.0), vec2(400.0, 400.0))
}

/// Where (x, y) lands seen [`from_above`], by hand.
fn above(x: f32, y: f32) -> Pos2 {
    pos2(210.0 + x, 220.0 - y)
}

fn paths(shapes: &[Shape]) -> Vec<&PathShape> {
    shapes
        .iter()
        .map(|shape| match shape {
            Shape::Path(path) => path,
            other => panic!("{other:?}"),
        })
        .collect()
}

/// The path's stroke colour at `at`.
fn colour_at(path: &PathShape, at: Pos2) -> Color32 {
    match &path.stroke.color {
        ColorMode::UV(colour) => colour(Rect::NOTHING, at),
        ColorMode::Solid(colour) => *colour,
    }
}

fn near(a: Pos2, b: Pos2) -> bool {
    (a - b).length() < 1e-3
}

/// The path whose ends land near `a` and `b`.
fn path_from<'a>(paths: &[&'a PathShape], a: Pos2, b: Pos2) -> &'a PathShape {
    paths
        .iter()
        .copied()
        .find(|p| p.points.len() == 2 && near(p.points[0], a) && near(p.points[1], b))
        .unwrap_or_else(|| panic!("no path from {a:?} to {b:?}"))
}

fn minor(alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(MINOR[0], MINOR[1], MINOR[2], alpha)
}

#[test]
fn from_above_each_stroke_lands_where_the_hand_puts_it_faded_along_it() {
    let drawn = shapes(&from_above(), SIZE, 1.0, rect());
    let paths = paths(&drawn);
    // x = 60 from (60, 80) to (60, 100): from (270, 140) to (270, 120); its alpha 95.5 at the
    // first end, 96, and 79.63 at the last, 80; halfway, their mean, 87.56, 88; past either
    // end (the stroke's cap, its smoothing), that end's.
    let at_100 = path_from(&paths, above(60.0, 80.0), above(60.0, 100.0));
    let [first, last] = [at_100.points[0], at_100.points[1]];
    assert_eq!(colour_at(at_100, first), minor(96));
    assert_eq!(colour_at(at_100, last), minor(80));
    assert_eq!(colour_at(at_100, first + (last - first) / 2.0), minor(88));
    assert_eq!(colour_at(at_100, first + vec2(0.0, 1.0)), minor(96));
    assert_eq!(colour_at(at_100, last - vec2(0.0, 1.0)), minor(80));
    assert_eq!(at_100.stroke.width, 1.0);
    assert!(!at_100.closed);
    // Off the fade, nothing: the corner's segment from (−200, −200) isn't painted.
    assert!(
        !paths
            .iter()
            .any(|p| near(p.points[0], above(-200.0, -200.0)))
    );
    // The axes last, each one colour along it, 1.5 points wide: X from (10, 220) to
    // (410, 220), Y from (210, 420) to (210, 20); Z, seen end on, isn't painted.
    let [.., x, y] = paths.as_slice() else {
        panic!("{} paths", paths.len());
    };
    let axes = [
        (x, above(-200.0, 0.0), above(200.0, 0.0), [0xff, 0x69, 0x81]),
        (y, above(0.0, -200.0), above(0.0, 200.0), [0x00, 0xb2, 0x66]),
    ];
    for (axis, from, to, [r, g, b]) in axes {
        assert!(
            near(axis.points[0], from) && near(axis.points[1], to),
            "{axis:?}"
        );
        let colour = Color32::from_rgba_unmultiplied(r, g, b, 230);
        for at in [from, to, from + (to - from) / 3.0] {
            assert_eq!(colour_at(axis, at), colour);
        }
        assert_eq!(axis.stroke.width, 1.5);
    }
    // Every stroke with an end not wholly faded (under 200 mm out, about), but Z: the
    // projection from above draws each whole and none end on but Z.
    let lit = strokes()
        .iter()
        .filter(|s| s.alpha.iter().any(|a| *a >= 0.5))
        .count();
    assert_eq!(paths.len(), lit - 1);
}

#[test]
fn from_the_front_the_plane_is_seen_edge_on_and_z_stands_up() {
    // Orthographic from −y, +z up, 400 mm high on 400 pixels: (x, z) lands at
    // (200 + x, 200 − z), from the corner (10, 20).
    let camera = Camera {
        projection: Projection::Orthographic { view_height: 400.0 },
        eye: [0.0, -100.0, 0.0],
        target: [0.0; 3],
        up: [0.0, 0.0, 1.0],
    };
    let drawn = shapes(&camera, SIZE, 1.0, rect());
    let paths = paths(&drawn);
    // Z last, from (210, 220) up to (210, 180); before it X, along the horizon from (10, 220)
    // to (410, 220). Y, and every line along y, is seen end on and isn't painted: the rest lie
    // along the horizon.
    let [rest @ .., x, z] = paths.as_slice() else {
        panic!("{} paths", paths.len());
    };
    assert!(near(z.points[0], pos2(210.0, 220.0)) && near(z.points[1], pos2(210.0, 180.0)));
    assert_eq!(
        colour_at(z, z.points[1]),
        Color32::from_rgba_unmultiplied(0x7a, 0x95, 0xff, 230)
    );
    assert!(near(x.points[0], pos2(10.0, 220.0)) && near(x.points[1], pos2(410.0, 220.0)));
    for path in rest {
        assert!(
            path.points.iter().all(|p| (p.y - 220.0).abs() < 1e-3),
            "{path:?}"
        );
        assert!(path.points[0].x < path.points[1].x, "{path:?}");
    }
}

#[test]
fn in_perspective_a_stroke_under_the_eye_is_cut_at_it_its_alpha_its_own_there() {
    // From (0, −30, 5) toward +y, +z up, 90°, on 100 by 100 pixels: (x, y, z) lands at
    // (50 + 50 x / d, 50 − 50 (z − 5) / d), d = y + 30 its depth. x = 10 from (10, −40) to
    // (10, −20) passes under the eye, at depths −10 to 10: drawn from (10 + 1e-6) / 20 of the
    // way on, its far end at (100, 75). Its alpha at the cut, between 191 (1 − √1700 / 200)
    // = 151.6243 and 191 (1 − √500 / 200) = 169.6456 at 0.50000005 of the way, is 160.6349,
    // 161; at its far end 170.
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 90.0,
        },
        eye: [0.0, -30.0, 5.0],
        target: [0.0, 0.0, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
    let drawn = shapes(&camera, [100, 100], 1.0, rect);
    let paths = paths(&drawn);
    let cut = paths
        .iter()
        .copied()
        .find(|p| near(p.points[1], pos2(100.0, 75.0)))
        .unwrap();
    // Its cut end lands far off to the right, 5e8 pixels out, on the line to its far end.
    assert!(cut.points[0].x > 1e8, "{cut:?}");
    assert_eq!(colour_at(cut, cut.points[0]), minor(161));
    assert_eq!(colour_at(cut, cut.points[1]), minor(170));
    // A stroke wholly behind the eye isn't painted: x = 10 from (10, −60) to (10, −40), at
    // depths −30 and −10, would land, taken through the projection uncut, mirrored across the
    // centre at (33.33, 41.67) and (0, 25), places no point of the grid (z = 0, below the
    // eye's 5) lands in front of it. No path ends there, and every point is finite.
    for mirrored in [pos2(100.0 / 3.0, 125.0 / 3.0), pos2(0.0, 25.0)] {
        assert!(
            !paths
                .iter()
                .any(|p| p.points.iter().any(|q| (*q - mirrored).length() < 1e-2)),
            "a path ends at {mirrored:?}"
        );
    }
    assert!(
        paths
            .iter()
            .all(|p| p.points.iter().all(|q| q.x.is_finite() && q.y.is_finite()))
    );
}

#[test]
fn at_two_pixels_a_point_each_stroke_lands_where_it_does_at_one() {
    // 400 mm on 800 by 800 pixels at 2 pixels a point, over the same 400-point rect: (x, y)
    // lands at pixel (400 + 2x, 400 − 2y), point (210 + x, 220 − y), as at one pixel a point.
    let drawn = shapes(&from_above(), [800, 800], 2.0, rect());
    let paths = paths(&drawn);
    let at_100 = path_from(&paths, above(60.0, 80.0), above(60.0, 100.0));
    assert_eq!(colour_at(at_100, at_100.points[0]), minor(96));
    assert_eq!(colour_at(at_100, at_100.points[1]), minor(80));
    assert_eq!(at_100.stroke.width, 1.0);
    let [.., x, _] = paths.as_slice() else {
        panic!("{} paths", paths.len());
    };
    assert!(near(x.points[0], above(-200.0, 0.0)) && near(x.points[1], above(200.0, 0.0)));
}

#[test]
fn a_view_the_renderer_refuses_paints_no_grid() {
    assert!(shapes(&from_above(), [0, 400], 1.0, rect()).is_empty());
    for ppp in [0.0, -1.0, f32::NAN] {
        assert!(shapes(&from_above(), SIZE, ppp, rect()).is_empty(), "{ppp}");
    }
    let refused = Camera {
        eye: [0.0; 3],
        ..from_above()
    };
    assert!(shapes(&refused, SIZE, 1.0, rect()).is_empty());
}
