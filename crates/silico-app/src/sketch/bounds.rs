//! The open sketch's geometry as a box in model space: what Fit, the named views, and Look
//! At take into their bounds in sketch mode, with the bodies', as SolidWorks' Zoom to Fit
//! takes every sketch entity in, so a sketch drawn beyond the bodies (or in a part with none)
//! fits (the M5.12 slice's finding, docs/walkthrough/m5.md). Its curves are the ones sketch
//! mode draws ([`Geometry`]), so the box and the drawing hold the same geometry.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use silico_core::command::sketch::SketchRead;

use super::draw::{Curve, Geometry, bounds};

/// The rectangle on its plane, in its own millimetres, that holds every point and every
/// circle and arc of `read`'s sketch (each line's ends are its points): none for a sketch
/// with nothing in it. A circle or an arc reaches each axis point its counter-clockwise sweep
/// passes, placed exactly, besides its ends.
pub fn plane_box(read: &SketchRead) -> Option<[[f64; 2]; 2]> {
    let geometry = Geometry::of(read);
    let mut reached: Vec<[f64; 2]> = geometry.points().map(|(_, at)| at).collect();
    for (_, curve, _) in geometry.curves() {
        let Curve::Round {
            centre,
            radius,
            start,
            sweep,
        } = curve
        else {
            continue;
        };
        // The axis points at 0, π/2, π, and 3π/2, each placed exactly, not by a cosine.
        let axes = [
            (0.0, [radius, 0.0]),
            (FRAC_PI_2, [0.0, radius]),
            (PI, [-radius, 0.0]),
            (PI + FRAC_PI_2, [0.0, -radius]),
        ];
        for (at_angle, offset) in axes {
            if (at_angle - start).rem_euclid(TAU) <= sweep {
                reached.push([centre[0] + offset[0], centre[1] + offset[1]]);
            }
        }
    }
    bounds(reached.into_iter()).map(|(low, high)| [low, high])
}

/// [`plane_box`] placed on the plane `read` placed its sketch on: the model-space box of the
/// rectangle's four corners, which holds every member, since each lies in that rectangle on
/// the plane. None if the read didn't place it (a sketch on a face not found), or if the
/// sketch has nothing in it.
pub fn model_box(read: &SketchRead) -> Option<[[f64; 3]; 2]> {
    let plane = *read.plane.as_ref().ok()?;
    let [[x0, y0], [x1, y1]] = plane_box(read)?;
    let corners = [[x0, y0], [x1, y0], [x0, y1], [x1, y1]].map(|corner| plane.place(corner));
    silico_core::views::joined(corners.map(|corner| [corner, corner]))
}

#[cfg(test)]
mod tests {
    use silico_core::document::{
        Coordinate, OriginPlane, Sketch, SketchArc, SketchCircle, SketchLine, SketchPoint,
    };
    use silico_protocol::{FaceFrame, SketchPlane};

    use super::*;
    use crate::sketch::tests::{id, point, under};

    fn read(points: Vec<SketchPoint>) -> SketchRead {
        under(Sketch {
            plane: OriginPlane::Xy.into(),
            points,
            lines: Vec::new(),
            circles: Vec::new(),
            arcs: Vec::new(),
            constraints: Vec::new(),
        })
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

    /// The slice's L: its six corners and the lines between them, on XY.
    fn l() -> SketchRead {
        let corners = [
            (0.0, 0.0),
            (60.0, 0.0),
            (60.0, 5.0),
            (5.0, 5.0),
            (5.0, 50.0),
            (0.0, 50.0),
        ];
        let mut l = read(
            corners
                .iter()
                .enumerate()
                .map(|(k, &(x, y))| point(&format!("pa00000{}", k + 1), x, y))
                .collect(),
        );
        l.members.lines = (1..=6)
            .map(|k| SketchLine {
                id: id(&format!("na00000{k}")),
                from: id(&format!("pa00000{k}")),
                to: id(&format!("pa00000{}", k % 6 + 1)),
                construction: false,
            })
            .collect();
        l
    }

    fn on(mut read: SketchRead, plane: SketchPlane) -> SketchRead {
        read.plane = Ok(plane);
        read
    }

    #[track_caller]
    fn near(a: [[f64; 3]; 2], b: [[f64; 3]; 2]) {
        for (k, (x, y)) in a.iter().flatten().zip(b.iter().flatten()).enumerate() {
            assert!((x - y).abs() <= 1e-12, "{k}: {a:?} against {b:?}");
        }
    }

    #[test]
    fn a_sketch_with_nothing_in_it_has_no_box() {
        assert_eq!(plane_box(&read(Vec::new())), None);
        assert_eq!(model_box(&on(read(Vec::new()), SketchPlane::Xz)), None);
    }

    #[test]
    fn a_sketch_its_read_didnt_place_has_no_model_box() {
        let mut unplaced = l();
        unplaced.plane = Err(silico_core::Error::new(
            silico_core::ErrorCode::ReferenceBroken,
            "the face isn't found",
        ));
        assert_eq!(plane_box(&unplaced), Some([[0.0, 0.0], [60.0, 50.0]]));
        assert_eq!(model_box(&unplaced), None);
    }

    #[test]
    fn the_l_spans_its_corners_and_lies_on_xz_and_yz_as_their_axes_place_it() {
        assert_eq!(plane_box(&l()), Some([[0.0, 0.0], [60.0, 50.0]]));
        // XZ: its x along +X, its y along +Z, at y = 0.
        assert_eq!(
            model_box(&on(l(), SketchPlane::Xz)),
            Some([[0.0, 0.0, 0.0], [60.0, 0.0, 50.0]])
        );
        // YZ: its x along +Y, its y along +Z, at x = 0.
        assert_eq!(
            model_box(&on(l(), SketchPlane::Yz)),
            Some([[0.0, 0.0, 0.0], [0.0, 60.0, 50.0]])
        );
    }

    #[test]
    fn on_a_face_turned_45_degrees_the_box_holds_all_four_corners_not_two() {
        // x along (1, 1, 0)/√2 and y along (−1, 1, 0)/√2 (y = n × x for n = +z). The L's
        // corners (0, 0), (60, 0), (0, 50), (60, 50) land at 0, 60(s, s, 0), 50(−s, s, 0),
        // and (10s, 110s, 0), s = 1/√2: x from −50s to 60s, y from 0 to 110s. The two
        // corners (0, 0) and (60, 50) alone would give x from 0 to 10s.
        let s = 1.0 / 2f64.sqrt();
        let face = SketchPlane::Face(FaceFrame {
            origin: [0.0; 3],
            x: [s, s, 0.0],
            y: [-s, s, 0.0],
        });
        near(
            model_box(&on(l(), face)).unwrap(),
            [[-50.0 * s, 0.0, 0.0], [60.0 * s, 110.0 * s, 0.0]],
        );
    }

    #[test]
    fn a_circle_reaches_its_radius_each_way_from_its_centre() {
        let mut r = read(vec![point("pa000001", 10.0, 20.0)]);
        r.members.circles = vec![SketchCircle {
            id: id("ca000001"),
            center: id("pa000001"),
            radius: Coordinate::new(5.0).unwrap(),
            construction: false,
        }];
        assert_eq!(plane_box(&r), Some([[5.0, 15.0], [15.0, 25.0]]));
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
        let mut quarter = read(points());
        quarter.members.arcs = vec![arc("pa000001", "pa000002", "pa000003")];
        assert_eq!(plane_box(&quarter), Some([[0.0, 0.0], [10.0, 10.0]]));
        // Three quarters from (0, 10) round through (−10, 0) and (0, −10) to (10, 0).
        let mut three = read(points());
        three.members.arcs = vec![arc("pa000001", "pa000003", "pa000002")];
        assert_eq!(plane_box(&three), Some([[-10.0, -10.0], [10.0, 10.0]]));
    }

    #[test]
    fn an_arc_off_the_origin_passing_one_axis_point_reaches_it_exactly() {
        // About (100, 100) from (103, 104) round to (97, 104): radius 5 (3, 4, 5), from
        // about 53° to 127°, so it passes 90°, the top, (100, 105), and no other axis point.
        // With its centre, the box is (97, 100) to (103, 105).
        let mut r = read(vec![
            point("pa000001", 100.0, 100.0),
            point("pa000002", 103.0, 104.0),
            point("pa000003", 97.0, 104.0),
        ]);
        r.members.arcs = vec![arc("pa000001", "pa000002", "pa000003")];
        assert_eq!(plane_box(&r), Some([[97.0, 100.0], [103.0, 105.0]]));
    }

    #[test]
    fn an_arc_starting_below_the_x_axis_wraps_round_to_the_bottom_axis_point() {
        // About the origin from (−3, −4), at about −127°, round to (3, −4), at about −53°:
        // it passes −90°, the bottom, (0, −5), whose angle 3π/2 is past the start only once
        // taken round a turn. With its centre, the box is (−3, −5) to (3, 0).
        let mut r = read(vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", -3.0, -4.0),
            point("pa000003", 3.0, -4.0),
        ]);
        r.members.arcs = vec![arc("pa000001", "pa000002", "pa000003")];
        assert_eq!(plane_box(&r), Some([[-3.0, -5.0], [3.0, 0.0]]));
    }

    #[test]
    fn an_arc_whose_ends_meet_sweeps_nothing_and_reaches_its_end_and_centre_alone() {
        // From (3, 4) to (3, 4): no sweep, as sketch mode draws it, so nothing past its end
        // and its centre.
        let mut r = read(vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 3.0, 4.0),
        ]);
        r.members.arcs = vec![arc("pa000001", "pa000002", "pa000002")];
        assert_eq!(plane_box(&r), Some([[0.0, 0.0], [3.0, 4.0]]));
    }
}
