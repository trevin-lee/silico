//! M5.9: a profile's faces on a face's frame (docs/ports/attacher.md, F1 and F2), not one of
//! the origin planes: on a box's top, on a frame tilted 30° about X, and on a reversed top
//! face, each face's area and centre where a hand calculation puts them, and its pad's and
//! revolve's volume and box by hand (Pappus for the revolve).

use super::close;

use std::f64::consts::PI;

use close::assert_close;
use silico_kernel::{
    Axis, Built, Curve, FaceFrame, FaceRegions, Region, SketchPlane, describe_whole, make_faces,
    prism, revolution, summarize,
};
use silico_protocol::{Description, Surface, radians};

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

/// The `w` × `h` rectangle from `(x, y)`, counter-clockwise from its lowest corner.
fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Vec<Curve> {
    let p = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect()
}

fn on(frame: FaceFrame, outer: Vec<Curve>) -> FaceRegions {
    FaceRegions {
        plane: SketchPlane::Face(frame),
        regions: vec![Region {
            outer,
            holes: vec![],
        }],
    }
}

/// The faces made, and their one face's description, checked.
fn made(regions: &FaceRegions) -> (Built, Description) {
    let built = make_faces(regions).unwrap();
    assert!(summarize(&built.shape).unwrap().valid);
    let d = describe_whole(&built.shape).unwrap();
    d.validate(1).unwrap();
    assert_eq!(d.faces.len(), 1);
    (built, d)
}

fn assert_point(what: &str, got: [f64; 3], want: [f64; 3], tolerance: f64) {
    let off = (0..3).map(|k| (got[k] - want[k]).abs()).fold(0.0, f64::max);
    assert!(off <= tolerance, "{what}: got {got:?}, want {want:?}");
}

/// The frame of a box's top face at z = 10: x +X, y +Y, n +Z, from (0, 0, 10).
fn top() -> FaceFrame {
    FaceFrame {
        origin: [0.0, 0.0, 10.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
    }
}

/// The XY frame tilted 30° about X, from `origin`: x +X, y (0, cos 30°, sin 30°), and n
/// (0, −sin 30°, cos 30°), which is x × y.
fn tilted(origin: [f64; 3]) -> FaceFrame {
    let (c, s) = (libm::cos(radians(30.0)), libm::sin(radians(30.0)));
    FaceFrame {
        origin,
        x: [1.0, 0.0, 0.0],
        y: [0.0, c, s],
        normal: [0.0, -s, c],
    }
}

#[test]
fn a_rectangle_on_a_boxs_top_face_is_at_the_faces_height_and_its_pad_rises_from_it() {
    // 20 × 10 at sketch (0..20, 0..10) on the top at z = 10: 200 mm², its centre
    // (10, 5, 10), on the plane z = 10 (the plane's own location there too, not at z = 0).
    // Padded 5 along n: 1000 mm³, from (0, 0, 10) to (20, 10, 15).
    let (faces, d) = made(&on(top(), rectangle(0.0, 0.0, 20.0, 10.0)));
    let face = &d.faces[0];
    assert_close("the face's area", face.area, 200.0, 1e-12);
    assert_point("the face's centre", face.centroid, [10.0, 5.0, 10.0], 1e-9);
    let Surface::Plane { normal, origin } = face.surface else {
        panic!("{:?}", face.surface)
    };
    assert_eq!(normal, [0.0, 0.0, 1.0]);
    assert_eq!(origin, [0.0, 0.0, 10.0]);
    let pad = prism(&faces.shape, [0.0; 3], [0.0, 0.0, 5.0]).unwrap();
    let s = summarize(&pad.shape).unwrap();
    assert!(s.valid);
    assert_close("the pad's volume", s.volume, 1000.0, 1e-12);
    assert_eq!((s.faces, s.edges, s.vertices), (6, 12, 8));
    let [low, high] = s.bounds.unwrap();
    assert_point("the pad's low corner", low, [0.0, 0.0, 10.0], 1e-9);
    assert_point("the pad's high corner", high, [20.0, 10.0, 15.0], 1e-9);
}

#[test]
fn a_rectangle_on_a_tilted_frame_lies_tilted_and_pads_along_its_normal() {
    // The same 20 × 10 on the frame tilted 30° about X from the origin: still 200 mm², its
    // centre (10, 5 cos 30°, 5 sin 30°) = (10, 4.330127…, 2.5). Padded 4 along n: 800 mm³,
    // its box from the faces' corners (0 and 10 along y) and n·4 on top: y from
    // −4 sin 30° = −2 to 10 cos 30°, z from 0 to 10 sin 30° + 4 cos 30°.
    let frame = tilted([0.0; 3]);
    let (c, s) = (3f64.sqrt() / 2.0, 0.5);
    let (faces, d) = made(&on(frame, rectangle(0.0, 0.0, 20.0, 10.0)));
    let face = &d.faces[0];
    assert_close("the face's area", face.area, 200.0, 1e-12);
    assert_point(
        "the face's centre",
        face.centroid,
        [10.0, 5.0 * c, 5.0 * s],
        1e-9,
    );
    let Surface::Plane { normal, origin } = face.surface else {
        panic!("{:?}", face.surface)
    };
    assert_point("the face's normal", normal, [0.0, -s, c], 1e-15);
    // The plane's own location is on it.
    let along: f64 = (0..3).map(|k| normal[k] * origin[k]).sum();
    assert!(along.abs() <= 1e-15, "{origin:?}");
    let pad = prism(&faces.shape, [0.0; 3], frame.normal.map(|k| k * 4.0)).unwrap();
    let summary = summarize(&pad.shape).unwrap();
    assert!(summary.valid);
    assert_close("the pad's volume", summary.volume, 800.0, 1e-9);
    assert_close(
        "the pad's area",
        summary.area,
        2.0 * 200.0 + 60.0 * 4.0,
        1e-9,
    );
    let [low, high] = summary.bounds.unwrap();
    assert_point("the pad's low corner", low, [0.0, -4.0 * s, 0.0], 1e-9);
    assert_point(
        "the pad's high corner",
        high,
        [20.0, 10.0 * c, 10.0 * s + 4.0 * c],
        1e-9,
    );
}

#[test]
fn a_circle_on_a_reversed_top_face_is_placed_mirrored_in_x() {
    // A top face reversed: n −Z, x −X, y +Y, from (0, 0, 7). A circle r 3 about sketch
    // (2, 1) is about (−2, 1, 7), 9π mm², its face facing −Z, its plane located at z = 7.
    let frame = FaceFrame {
        origin: [0.0, 0.0, 7.0],
        x: [-1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
        normal: [0.0, 0.0, -1.0],
    };
    let (faces, d) = made(&on(
        frame,
        vec![Curve::Circle {
            center: [2.0, 1.0],
            radius: 3.0,
        }],
    ));
    let face = &d.faces[0];
    assert_close("the face's area", face.area, 9.0 * PI, 1e-12);
    assert_point("the face's centre", face.centroid, [-2.0, 1.0, 7.0], 1e-9);
    let Surface::Plane { normal, origin } = face.surface else {
        panic!("{:?}", face.surface)
    };
    assert_eq!(normal, [0.0, 0.0, -1.0]);
    assert_eq!(origin, [0.0, 0.0, 7.0]);
    // The circle's one vertex, at its angle 0 along the sketch's x: (2 + 3, 1) on the
    // sketch, (−5, 1, 7) in the model.
    assert_eq!(d.vertices.len(), 1);
    assert_point(
        "the circle's vertex",
        d.vertices[0],
        [-5.0, 1.0, 7.0],
        1e-12,
    );
    // Padded 2 along n, down: 18π mm³, from z = 5 to 7, x from −5 to 1.
    let pad = prism(&faces.shape, [0.0; 3], [0.0, 0.0, -2.0]).unwrap();
    let s = summarize(&pad.shape).unwrap();
    assert!(s.valid);
    assert_close("the pad's volume", s.volume, 18.0 * PI, 1e-12);
    let [low, high] = s.bounds.unwrap();
    assert_point("the pad's low corner", low, [-5.0, -2.0, 5.0], 1e-9);
    assert_point("the pad's high corner", high, [1.0, 4.0, 7.0], 1e-9);
}

#[test]
fn a_revolve_on_a_tilted_frame_about_its_y_axis_is_pappus_s() {
    // On the frame tilted 30° about X from (1, 2, 3): a 2 × 3 rectangle at sketch u 2..4,
    // v 0..3, turned about the frame's y axis through its origin (the sketch's u = 0, in
    // its plane). Pappus: its area 6 times the path of its centre, 3 from the axis, so a
    // quarter turn sweeps 6 · 3 · π/2 = 9π mm³ and a whole one 6 · 3 · 2π = 36π.
    let frame = tilted([1.0, 2.0, 3.0]);
    let (faces, _) = made(&on(frame, rectangle(2.0, 0.0, 2.0, 3.0)));
    let axis = Axis {
        origin: frame.origin,
        direction: frame.y,
    };
    for (angle, want) in [(radians(90.0), 9.0 * PI), (radians(360.0), 36.0 * PI)] {
        let turned = revolution(&faces.shape, &axis, 0.0, angle).unwrap();
        let s = summarize(&turned.shape).unwrap();
        assert!(s.valid, "{angle}");
        assert_close("the revolve's volume", s.volume, want, 1e-9);
    }
    // A whole turn's box: a ring about the axis, 4 out, 3 along it. Along the axis from
    // (1, 2, 3) to 3 up it, and out to 4 every way across it: along X, 1 ± 4.
    let whole = revolution(&faces.shape, &axis, 0.0, radians(360.0)).unwrap();
    let [low, high] = summarize(&whole.shape).unwrap().bounds.unwrap();
    assert_close("the ring's low x", low[0], -3.0, 1e-9);
    assert_close("the ring's high x", high[0], 5.0, 1e-9);
}
