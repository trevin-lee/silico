//! The open sketch's geometry as a box in model space: what Fit, the named views, and Look
//! At take into their bounds in sketch mode, with the bodies', as SolidWorks' Zoom to Fit
//! takes every sketch entity in, so a sketch drawn beyond the bodies (or in a part with none)
//! fits.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use silico_core::document::{ElementId, Sketch};
use silico_protocol::SketchPlane;

/// The rectangle on its plane, in its own millimetres, that holds every point, circle, and
/// arc of `sketch` (each line's ends are its points): none for a sketch with nothing in it.
/// An arc reaches its ends and each axis point its counter-clockwise sweep passes, about its
/// centre at the centre's distance to its start, as the core takes an arc.
pub fn plane_box(sketch: &Sketch) -> Option<[[f64; 2]; 2]> {
    let at: BTreeMap<&ElementId, [f64; 2]> = sketch
        .points
        .iter()
        .map(|point| (&point.id, [point.x.mm(), point.y.mm()]))
        .collect();
    let mut reached: Vec<[f64; 2]> = at.values().copied().collect();
    for circle in &sketch.circles {
        if let Some(&[x, y]) = at.get(&circle.center) {
            let r = circle.radius.mm();
            reached.extend([[x - r, y - r], [x + r, y + r]]);
        }
    }
    for arc in &sketch.arcs {
        let (Some(&centre), Some(&start), Some(&end)) =
            (at.get(&arc.center), at.get(&arc.start), at.get(&arc.end))
        else {
            continue;
        };
        let from = angle(sub(start, centre));
        let sweep = (angle(sub(end, centre)) - from).rem_euclid(TAU);
        let r = length(sub(start, centre));
        // The axis points at 0, π/2, π, and 3π/2, each placed exactly, not by a cosine.
        let axes = [
            (0.0, [r, 0.0]),
            (FRAC_PI_2, [0.0, r]),
            (PI, [-r, 0.0]),
            (PI + FRAC_PI_2, [0.0, -r]),
        ];
        for (at_angle, offset) in axes {
            if (at_angle - from).rem_euclid(TAU) <= sweep {
                reached.push([centre[0] + offset[0], centre[1] + offset[1]]);
            }
        }
    }
    let first = *reached.first()?;
    Some(reached.iter().fold([first, first], |[low, high], p| {
        [
            [low[0].min(p[0]), low[1].min(p[1])],
            [high[0].max(p[0]), high[1].max(p[1])],
        ]
    }))
}

/// `sketch`'s [`plane_box`] placed on `plane`: the model-space box of its four corners, which
/// holds every member, since each lies in that rectangle on the plane.
pub fn model_box(sketch: &Sketch, plane: SketchPlane) -> Option<[[f64; 3]; 2]> {
    let [[x0, y0], [x1, y1]] = plane_box(sketch)?;
    let corners = [[x0, y0], [x1, y0], [x0, y1], [x1, y1]].map(|corner| plane.place(corner));
    let first = corners[0];
    Some(corners.iter().fold([first, first], |[low, high], p| {
        [
            std::array::from_fn(|k| low[k].min(p[k])),
            std::array::from_fn(|k| high[k].max(p[k])),
        ]
    }))
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn length(a: [f64; 2]) -> f64 {
    libm::hypot(a[0], a[1])
}

fn angle(a: [f64; 2]) -> f64 {
    libm::atan2(a[1], a[0])
}

#[cfg(test)]
mod tests {
    use silico_core::document::{
        Coordinate, OriginPlane, SketchArc, SketchCircle, SketchLine, SketchPoint,
    };

    use super::*;

    fn id(text: &str) -> ElementId {
        ElementId::new(text).unwrap()
    }

    fn point(name: &str, x: f64, y: f64) -> SketchPoint {
        SketchPoint {
            id: id(name),
            x: Coordinate::new(x).unwrap(),
            y: Coordinate::new(y).unwrap(),
            construction: false,
        }
    }

    fn sketch(points: Vec<SketchPoint>) -> Sketch {
        Sketch {
            plane: OriginPlane::Xy.into(),
            points,
            lines: Vec::new(),
            circles: Vec::new(),
            arcs: Vec::new(),
            constraints: Vec::new(),
        }
    }

    fn arc(centre: &str, start: &str, end: &str) -> SketchArc {
        SketchArc {
            id: id("aa000001"),
            center: id(centre),
            start: id(start),
            end: id(end),
            construction: false,
        }
    }

    /// The slice's L: its six corners and the lines between them.
    fn l() -> Sketch {
        let corners = [
            (0.0, 0.0),
            (60.0, 0.0),
            (60.0, 5.0),
            (5.0, 5.0),
            (5.0, 50.0),
            (0.0, 50.0),
        ];
        let mut l = sketch(
            corners
                .iter()
                .enumerate()
                .map(|(k, &(x, y))| point(&format!("pa00000{}", k + 1), x, y))
                .collect(),
        );
        l.lines = (1..=6)
            .map(|k| SketchLine {
                id: id(&format!("na00000{k}")),
                from: id(&format!("pa00000{k}")),
                to: id(&format!("pa00000{}", k % 6 + 1)),
                construction: false,
            })
            .collect();
        l
    }

    #[test]
    fn a_sketch_with_nothing_in_it_has_no_box() {
        assert_eq!(plane_box(&sketch(Vec::new())), None);
        assert_eq!(model_box(&sketch(Vec::new()), SketchPlane::Xz), None);
    }

    #[test]
    fn the_l_spans_its_corners_and_lies_on_xz_and_yz_as_their_axes_place_it() {
        assert_eq!(plane_box(&l()), Some([[0.0, 0.0], [60.0, 50.0]]));
        // XZ: its x along +X, its y along +Z, at y = 0.
        assert_eq!(
            model_box(&l(), SketchPlane::Xz),
            Some([[0.0, 0.0, 0.0], [60.0, 0.0, 50.0]])
        );
        // YZ: its x along +Y, its y along +Z, at x = 0.
        assert_eq!(
            model_box(&l(), SketchPlane::Yz),
            Some([[0.0, 0.0, 0.0], [0.0, 60.0, 50.0]])
        );
    }

    #[test]
    fn a_circle_reaches_its_radius_each_way_from_its_centre() {
        let mut s = sketch(vec![point("pa000001", 10.0, 20.0)]);
        s.circles = vec![SketchCircle {
            id: id("ca000001"),
            center: id("pa000001"),
            radius: Coordinate::new(5.0).unwrap(),
            construction: false,
        }];
        assert_eq!(plane_box(&s), Some([[5.0, 15.0], [15.0, 25.0]]));
    }

    #[test]
    fn an_arc_reaches_the_axis_points_its_sweep_passes_and_no_others() {
        let points = || {
            vec![
                point("pa000001", 0.0, 0.0),
                point("pa000002", 10.0, 0.0),
                point("pa000003", 0.0, 10.0),
            ]
        };
        // A quarter from (10, 0) round to (0, 10): its ends and its centre, nothing past.
        let mut quarter = sketch(points());
        quarter.arcs = vec![arc("pa000001", "pa000002", "pa000003")];
        assert_eq!(plane_box(&quarter), Some([[0.0, 0.0], [10.0, 10.0]]));
        // Three quarters from (0, 10) round through (−10, 0) and (0, −10) to (10, 0).
        let mut three = sketch(points());
        three.arcs = vec![arc("pa000001", "pa000003", "pa000002")];
        assert_eq!(plane_box(&three), Some([[-10.0, -10.0], [10.0, 10.0]]));
    }

    #[test]
    fn an_arc_off_the_origin_passing_one_axis_point_reaches_it_exactly() {
        // About (100, 100) from (103, 104) round to (97, 104): radius 5 (3, 4, 5), from
        // about 53° to 127°, so it passes 90°, the top, (100, 105), and no other axis point.
        // With its centre, the box is (97, 100) to (103, 105).
        let mut s = sketch(vec![
            point("pa000001", 100.0, 100.0),
            point("pa000002", 103.0, 104.0),
            point("pa000003", 97.0, 104.0),
        ]);
        s.arcs = vec![arc("pa000001", "pa000002", "pa000003")];
        assert_eq!(plane_box(&s), Some([[97.0, 100.0], [103.0, 105.0]]));
    }
}
