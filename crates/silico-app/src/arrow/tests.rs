//! The dialog's arrows: the profile's centroid, each arrow's place and way in model space and
//! on the screen, its hit area, a drag's value, the flip, and a frame's events, each worked by
//! hand. On the screen the views are orthographic from the front (or from 45° above it, or
//! square to a tilted face), 50 mm high over an image 400 by 200 pixels at 2 pixels a point
//! with its top left at (40, 30) points: 4 pixels, 2 points, a millimetre, the target at the
//! image's centre, pixel (200, 100), point (140, 80). The renderer's projection is f32's, so
//! places on the screen are compared to a thousandth of a point.

use std::f64::consts::{FRAC_1_SQRT_2, PI, SQRT_2};

use eframe::egui::{Color32, Pos2, Shape, pos2};
use silico_core::command::sketch::SketchRead;
use silico_core::document::{
    Coordinate, FeatureId, Length, OriginPlane, Sketch, SketchCircle, SketchPoint,
};
use silico_core::kernel::SketchPlane;
use silico_protocol::{Camera, FaceFrame, Projection};

use super::*;
use crate::dialog::{Dialog, Kind, Part, Value};
use crate::sketch::tests::{id, line, point, rectangle, region, under, with_painter};
use crate::viewport::{LeftEvent, LeftKind};

const ORIGIN: Pos2 = pos2(40.0, 30.0);
const SIZE: [u32; 2] = [400, 200];
const PPP: f32 = 2.0;
const LENGTH: Part = Part::ExtentNumber("length");
const LENGTH2: Part = Part::ExtentNumber("length2");

fn mm(text: &str) -> Length {
    Length::parse(text).unwrap()
}

#[track_caller]
fn near(got: [f64; 2], expected: [f64; 2], within: f64) {
    for k in 0..2 {
        assert!(
            (got[k] - expected[k]).abs() <= within,
            "{got:?} against {expected:?}, within {within}"
        );
    }
}

#[track_caller]
fn near3(got: [f64; 3], expected: [f64; 3]) {
    for k in 0..3 {
        assert!(
            (got[k] - expected[k]).abs() <= 1e-9,
            "{got:?} against {expected:?}"
        );
    }
}

/// `got` on the screen at `expected`, to a thousandth of a point.
#[track_caller]
fn at(got: Pos2, expected: [f64; 2]) {
    near([f64::from(got.x), f64::from(got.y)], expected, 1e-3);
}

/// An orthographic view 50 mm high from `toward` (from the target toward the eye), `up`.
fn ortho(target: [f64; 3], toward: [f64; 3], up: [f64; 3]) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 50.0 },
        eye: std::array::from_fn(|k| target[k] + 500.0 * toward[k]),
        target,
        up,
    }
}

/// [`ortho`] from the front, Z up: X right, Z up the screen.
fn front(target: [f64; 3]) -> Camera {
    ortho(target, [0.0, -1.0, 0.0], [0.0, 0.0, 1.0])
}

fn on_screen(handles: &Handles, camera: &Camera) -> Arrows {
    arrows(handles, camera, SIZE, PPP, ORIGIN)
}

/// A sweep's dialog of `kind` on the sketch s0000001, its inputs at their defaults (a pad's
/// length 10, a pocket's 5, one side, not reversed).
fn dialog_of(kind: Kind) -> Dialog {
    Dialog::new(kind, Some(FeatureId::new("s0000001").unwrap()), 7)
}

/// `dialog`'s input `part` holding `text`.
fn set_text(dialog: &mut Dialog, part: Part, text: &str) {
    let input = dialog.inputs.iter_mut().find(|i| i.part == part).unwrap();
    let Value::Length(held) = &mut input.value else {
        panic!("not a length");
    };
    *held = text.to_owned();
}

fn text_of(dialog: &Dialog, part: Part) -> String {
    match &dialog.inputs.iter().find(|i| i.part == part).unwrap().value {
        Value::Length(text) => text.clone(),
        _ => panic!("not a length"),
    }
}

/// `dialog`'s extent made the form `form`.
fn set_form(dialog: &mut Dialog, form: &str) {
    let input = dialog
        .inputs
        .iter_mut()
        .find(|i| i.part == Part::ExtentType)
        .unwrap();
    let Value::Choice { options, chosen } = &mut input.value else {
        panic!("not a choice");
    };
    *chosen = options.iter().position(|(value, _)| value == form).unwrap();
}

fn set_reversed(dialog: &mut Dialog, on: bool) {
    let input = dialog
        .inputs
        .iter_mut()
        .find(|i| i.part == Part::Reversed)
        .unwrap();
    input.value = Value::Toggle(on);
}

fn reversed(dialog: &Dialog) -> bool {
    matches!(
        dialog
            .inputs
            .iter()
            .find(|i| i.part == Part::Reversed)
            .unwrap()
            .value,
        Value::Toggle(true)
    )
}

/// The 40 × 20 rectangle at the origin, its profile its one region: centroid (20, 10).
fn rectangle_read() -> SketchRead {
    let mut read = under(rectangle(Vec::new()));
    read.regions = vec![region(
        &["na000001", "na000002", "na000003", "na000004"],
        &[],
    )];
    read
}

/// A sketch of `points` joined in order by lines nb000001…, its one region their loop.
fn polygon(points: &[(f64, f64)]) -> SketchRead {
    let names: Vec<String> = (1..=points.len()).map(|n| format!("pb{n:06}")).collect();
    let lines: Vec<String> = (1..=points.len()).map(|n| format!("nb{n:06}")).collect();
    let sketch = Sketch {
        plane: OriginPlane::Xy.into(),
        points: points
            .iter()
            .zip(&names)
            .map(|((x, y), name)| point(name, *x, *y))
            .collect(),
        lines: (0..points.len())
            .map(|k| line(&lines[k], &names[k], &names[(k + 1) % points.len()]))
            .collect(),
        circles: Vec::new(),
        arcs: Vec::new(),
        constraints: Vec::new(),
    };
    let mut read = under(sketch);
    let loop_: Vec<&str> = lines.iter().map(String::as_str).collect();
    read.regions = vec![region(&loop_, &[])];
    read
}

/// The rectangle's pad's arrows, its sketch on `plane` (its centroid at (20, 10) there).
fn pad_handles(dialog: &Dialog, plane: SketchPlane) -> Handles {
    handles(dialog, plane, [20.0, 10.0]).unwrap()
}

// ---- The profile's centroid ----

#[test]
fn a_rectangles_centroid_is_its_middle_exactly() {
    // About the middle of its bounds, (20, 10): each corner ±20, ±10, each side's moment
    // cancelling its opposite's, so (20, 10) to the bit.
    assert_eq!(profile_centroid(&rectangle_read()), Some([20.0, 10.0]));
}

#[test]
fn the_prototypes_bracket_profiles_centroid_is_where_it_puts_its_arrow_to_round_numbers() {
    // The prototype's L, W 60, tb 5, tu 5, H 50 (geometry.js's `prof`, without its fillet): a
    // 60 × 5 foot, 300 mm² about (30, 2.5), and a 5 × 45 upright, 225 mm² about (2.5, 27.5).
    // (300·30 + 225·2.5) / 525 = 18.2142857…, (300·2.5 + 225·27.5) / 525 = 13.2142857…:
    // the prototype's `(18, D, 14)`.
    let read = polygon(&[
        (0.0, 0.0),
        (60.0, 0.0),
        (60.0, 5.0),
        (5.0, 5.0),
        (5.0, 50.0),
        (0.0, 50.0),
    ]);
    near(
        profile_centroid(&read).unwrap(),
        [9562.5 / 525.0, 6937.5 / 525.0],
        1e-12,
    );
}

#[test]
fn a_hole_takes_its_area_away_whichever_way_its_circle_runs() {
    // The rectangle less a circle of radius 5 about (10, 10), which runs counter-clockwise
    // as a lone circle walks, a hole all the same: (800·20 − 25π·10) / (800 − 25π) across,
    // 10 up. The circle's chords stray at most a micrometre, so its area is short by at most
    // its perimeter times that, 0.0314 mm², which moves the centroid by at most
    // 0.0314 · 11.1 / 721 = 5e-4 mm across, and not at all up (its chords are symmetric).
    let mut read = rectangle_read();
    read.members.points.push(point("pc000001", 10.0, 10.0));
    read.members.circles.push(SketchCircle {
        id: id("ca000001"),
        center: id("pc000001"),
        radius: Coordinate::new(5.0).unwrap(),
        construction: false,
    });
    read.regions = vec![region(
        &["na000001", "na000002", "na000003", "na000004"],
        &[&["ca000001"]],
    )];
    let hole = 25.0 * PI;
    let centroid = profile_centroid(&read).unwrap();
    near(
        centroid,
        [(16_000.0 - 10.0 * hole) / (800.0 - hole), 10.0],
        1e-3,
    );
    // Not the rectangle's, which a hole added would move the other way.
    assert!(centroid[0] > 21.0, "{centroid:?}");
}

#[test]
fn two_regions_centroid_is_their_areas_together() {
    // Squares [0, 10]² and [30, 40] × [0, 10]: (20, 5).
    let mut read = polygon(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    let far = polygon(&[(30.0, 0.0), (40.0, 0.0), (40.0, 10.0), (30.0, 10.0)]);
    for (p, q) in far
        .members
        .points
        .iter()
        .zip(["pd000001", "pd000002", "pd000003", "pd000004"])
    {
        read.members.points.push(SketchPoint {
            id: id(q),
            ..p.clone()
        });
    }
    let ends = ["pd000001", "pd000002", "pd000003", "pd000004"];
    let sides = ["nd000001", "nd000002", "nd000003", "nd000004"];
    for k in 0..4 {
        read.members
            .lines
            .push(line(sides[k], ends[k], ends[(k + 1) % 4]));
    }
    read.regions.push(region(&sides, &[]));
    near(profile_centroid(&read).unwrap(), [20.0, 5.0], 1e-12);
}

#[test]
fn a_profile_far_from_the_origin_keeps_its_own_precision() {
    // 2 µm by 1 µm at (1e5, 1e5): its centroid (1e5 + 1e-3, 1e5 + 5e-4). Taken about the
    // sketch's origin, each cross product is 1e10 against an area of 2e-6: the sum would
    // lose every digit of it.
    let read = polygon(&[
        (100_000.0, 100_000.0),
        (100_000.002, 100_000.0),
        (100_000.002, 100_000.001),
        (100_000.0, 100_000.001),
    ]);
    near(
        profile_centroid(&read).unwrap(),
        [100_000.001, 100_000.000_5],
        1e-9,
    );
}

#[test]
fn a_sketch_with_no_profile_or_none_with_area_has_no_centroid() {
    // No region: the read's profile problems say why.
    assert_eq!(profile_centroid(&under(rectangle(Vec::new()))), None);
    // A region naming a curve the read hasn't.
    let mut read = rectangle_read();
    read.regions = vec![region(&["na000001", "na000002", "nz000009"], &[])];
    assert_eq!(profile_centroid(&read), None);
    // Three points on a line: a loop of no area.
    let flat = polygon(&[(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)]);
    assert_eq!(profile_centroid(&flat), None);
}

// ---- Model space ----

#[test]
fn a_pads_arrow_stands_at_its_end_over_the_centroid_and_its_flip_on_the_plane() {
    // On XY, the normal +Z: the pad's length 10 from (20, 10, 0) to (20, 10, 10).
    let h = pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xy);
    near3(h.start, [20.0, 10.0, 0.0]);
    near3(h.direction, [0.0, 0.0, 1.0]);
    assert_eq!(h.distances.len(), 1);
    let d = h.distances[0];
    assert_eq!((d.part, d.value, d.factor), (LENGTH, mm("10"), 1.0));
    near3(d.base, [20.0, 10.0, 10.0]);
    near3(d.way, [0.0, 0.0, 1.0]);
    assert!(h.flip);
    // Reversed: below the plane, pointing down, the flip arrow with it.
    let mut dialog = dialog_of(Kind::Pad);
    set_reversed(&mut dialog, true);
    let h = pad_handles(&dialog, SketchPlane::Xy);
    near3(h.direction, [0.0, 0.0, -1.0]);
    near3(h.distances[0].base, [20.0, 10.0, -10.0]);
    near3(h.distances[0].way, [0.0, 0.0, -1.0]);
    // On XZ, whose normal is −Y: (20, 0, 10) placed, the end at (20, −10, 10).
    let h = pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xz);
    near3(h.start, [20.0, 0.0, 10.0]);
    near3(h.distances[0].base, [20.0, -10.0, 10.0]);
}

#[test]
fn a_pockets_arrow_goes_against_the_normal_and_through_all_has_only_the_flip() {
    // A pocket's default length 5, its way −Z on XY.
    let h = pad_handles(&dialog_of(Kind::Pocket), SketchPlane::Xy);
    near3(h.direction, [0.0, 0.0, -1.0]);
    near3(h.distances[0].base, [20.0, 10.0, -5.0]);
    assert!(h.flip);
    let mut dialog = dialog_of(Kind::Pocket);
    set_reversed(&mut dialog, true);
    near3(
        pad_handles(&dialog, SketchPlane::Xy).distances[0].base,
        [20.0, 10.0, 5.0],
    );
    // Through all: no length, Reversed shown.
    let mut dialog = dialog_of(Kind::Pocket);
    set_form(&mut dialog, "through_all");
    let h = pad_handles(&dialog, SketchPlane::Xy);
    assert!(h.distances.is_empty());
    assert!(h.flip);
    near3(h.direction, [0.0, 0.0, -1.0]);
}

#[test]
fn a_symmetric_extents_arrow_stands_at_half_its_length_and_two_sides_have_one_each() {
    // Symmetric 10: the end at 5, each millimetre it moves two of the length; no flip.
    let mut dialog = dialog_of(Kind::Pad);
    set_form(&mut dialog, "symmetric");
    set_reversed(&mut dialog, true);
    let h = pad_handles(&dialog, SketchPlane::Xy);
    assert_eq!(h.distances.len(), 1);
    near3(h.distances[0].base, [20.0, 10.0, 5.0]);
    assert_eq!(h.distances[0].factor, 2.0);
    // Reversed is hidden for symmetric, so it changes nothing.
    near3(h.direction, [0.0, 0.0, 1.0]);
    assert!(!h.flip);
    // Two sides, 10 and 4: one up to 10, one down to −4, pointing down.
    let mut dialog = dialog_of(Kind::Pad);
    set_form(&mut dialog, "two_sides");
    set_text(&mut dialog, LENGTH2, "4");
    let h = pad_handles(&dialog, SketchPlane::Xy);
    let parts: Vec<Part> = h.distances.iter().map(|d| d.part).collect();
    assert_eq!(parts, [LENGTH, LENGTH2]);
    near3(h.distances[0].base, [20.0, 10.0, 10.0]);
    near3(h.distances[1].base, [20.0, 10.0, -4.0]);
    near3(h.distances[1].way, [0.0, 0.0, -1.0]);
    assert_eq!(h.distances[1].value, mm("4"));
    assert!(!h.flip);
}

#[test]
fn a_length_that_isnt_one_above_zero_has_no_arrow_and_other_kinds_have_none() {
    for text in ["", "abc", "-5", "0", "-0", "1e400", "0.0000001", "10x"] {
        let mut dialog = dialog_of(Kind::Pad);
        set_text(&mut dialog, LENGTH, text);
        let h = pad_handles(&dialog, SketchPlane::Xy);
        assert!(h.distances.is_empty(), "{text:?}: {h:?}");
        // The flip arrow still: Reversed shows.
        assert!(h.flip, "{text:?}");
    }
    // Exactly as the document reads it: an exponent, space around it.
    let mut dialog = dialog_of(Kind::Pad);
    set_text(&mut dialog, LENGTH, " 1.25e1 ");
    let h = pad_handles(&dialog, SketchPlane::Xy);
    assert_eq!(h.distances[0].value, mm("12.5"));
    near3(h.distances[0].base, [20.0, 10.0, 12.5]);
    for kind in [
        Kind::Revolve,
        Kind::Groove,
        Kind::Fillet,
        Kind::Union,
        Kind::Box,
    ] {
        assert_eq!(
            handles(&dialog_of(kind), SketchPlane::Xy, [20.0, 10.0]),
            None
        );
    }
    // A centroid that isn't a place.
    assert_eq!(
        handles(&dialog_of(Kind::Pad), SketchPlane::Xy, [f64::NAN, 0.0]),
        None
    );
}

// ---- On the screen ----

#[test]
fn from_the_front_the_arrow_points_up_56_points_from_its_end_as_worked_by_hand() {
    // The end (20, 10, 10) at the target: point (140, 80). +Z is up the screen at 2 points a
    // millimetre: the tip 56 up, (140, 24); the head's point 12 past it, (140, 12), its
    // corners 6 either side, (146, 24) and (134, 24); the label's box from 22 past the tip
    // and 6 right, (146, 2). The flip arrow from (20, 10, 0), 10 mm below the target, (140,
    // 100), 28 up to (140, 72).
    let h = pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xy);
    let a = on_screen(&h, &front([20.0, 10.0, 10.0]));
    assert_eq!(a.distances.len(), 1);
    let d = &a.distances[0];
    at(d.arrow.base, [140.0, 80.0]);
    near(d.arrow.unit, [0.0, -1.0], 1e-6);
    assert!((d.arrow.per_mm - 2.0).abs() < 1e-5, "{}", d.arrow.per_mm);
    at(d.arrow.tip, [140.0, 24.0]);
    at(d.arrow.head[0], [140.0, 12.0]);
    at(d.arrow.head[1], [146.0, 24.0]);
    at(d.arrow.head[2], [134.0, 24.0]);
    at(d.label_at, [146.0, 2.0]);
    assert_eq!(d.label, "10 mm");
    let flip = a.flip.unwrap();
    at(flip.base, [140.0, 100.0]);
    at(flip.tip, [140.0, 72.0]);
    at(flip.head[0], [140.0, 60.0]);
}

#[test]
fn on_a_tilted_face_the_arrow_points_along_its_normal_on_the_screen() {
    // A face whose normal is (1, 0, 1)/√2, its x (1, 0, −1)/√2 and y +Y, at the origin: the
    // centroid (20, 10) at (20/√2, 10, −20/√2), the end 10 along the normal at
    // (30/√2, 10, −10/√2), the target. From the front the normal lies in the screen, up and
    // right: unit (1/√2, −1/√2), 2 points a millimetre; the tip 56 along it, (140 + 56/√2,
    // 80 − 56/√2); the head's point 68 along; its corners 6 either side, across (1/√2, 1/√2);
    // the label from 78 along and 6 right.
    let r = FRAC_1_SQRT_2;
    let plane = SketchPlane::Face(FaceFrame {
        origin: [0.0; 3],
        x: [r, 0.0, -r],
        y: [0.0, 1.0, 0.0],
    });
    let h = pad_handles(&dialog_of(Kind::Pad), plane);
    near3(h.distances[0].base, [30.0 * r, 10.0, -10.0 * r]);
    let a = on_screen(&h, &front(h.distances[0].base));
    let d = &a.distances[0];
    at(d.arrow.base, [140.0, 80.0]);
    near(d.arrow.unit, [r, -r], 1e-6);
    assert!((d.arrow.per_mm - 2.0).abs() < 1e-5);
    at(d.arrow.tip, [140.0 + 56.0 * r, 80.0 - 56.0 * r]);
    at(d.arrow.head[0], [140.0 + 68.0 * r, 80.0 - 68.0 * r]);
    at(d.arrow.head[1], [140.0 + 62.0 * r, 80.0 - 50.0 * r]);
    at(d.arrow.head[2], [140.0 + 50.0 * r, 80.0 - 62.0 * r]);
    at(d.label_at, [146.0 + 78.0 * r, 80.0 - 78.0 * r]);
}

#[test]
fn from_45_degrees_above_the_sweep_is_foreshortened_by_its_cosine() {
    // From (0, −1, 1): the view's up (0, 1, 1)/√2, so +Z is 1/√2 of itself up the screen, √2
    // points a millimetre; the arrow is still 56 points long.
    let target = [20.0, 10.0, 10.0];
    let camera = ortho(target, [0.0, -r2(), r2()], [0.0, 0.0, 1.0]);
    let a = on_screen(
        &pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xy),
        &camera,
    );
    let d = &a.distances[0];
    at(d.arrow.base, [140.0, 80.0]);
    near(d.arrow.unit, [0.0, -1.0], 1e-6);
    assert!((d.arrow.per_mm - SQRT_2).abs() < 1e-5, "{}", d.arrow.per_mm);
    at(d.arrow.tip, [140.0, 24.0]);
    // The flip arrow's base, 10 mm down Z: 10/√2 mm down the view, 10√2 points.
    at(a.flip.unwrap().base, [140.0, 80.0 + 10.0 * SQRT_2]);
}

fn r2() -> f64 {
    FRAC_1_SQRT_2
}

#[test]
fn in_perspective_the_arrow_is_measured_at_its_end() {
    // 90° high from 100 mm in front of the end, an image 200 square at a pixel a point: a
    // place in the end's plane square to the view moves a pixel a millimetre there (100 over
    // 100 · tan 45°). The end at the centre, (100, 100); +Z up at 1 point a millimetre.
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 90.0,
        },
        eye: [20.0, -90.0, 10.0],
        target: [20.0, 10.0, 10.0],
        up: [0.0, 0.0, 1.0],
    };
    let h = pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xy);
    let a = arrows(&h, &camera, [200, 200], 1.0, pos2(0.0, 0.0));
    let d = &a.distances[0];
    at(d.arrow.base, [100.0, 100.0]);
    near(d.arrow.unit, [0.0, -1.0], 1e-6);
    assert!((d.arrow.per_mm - 1.0).abs() < 1e-5, "{}", d.arrow.per_mm);
    at(d.arrow.tip, [100.0, 44.0]);
    // The eye 5 mm past the end, looking away from it: neither arrow can be shown.
    let behind = Camera {
        eye: [20.0, 15.0, 10.0],
        target: [20.0, 115.0, 10.0],
        ..camera
    };
    assert!(arrows(&h, &behind, [200, 200], 1.0, pos2(0.0, 0.0)).is_empty());
}

#[test]
fn seen_end_on_or_under_six_tenths_of_a_point_a_millimetre_no_arrow_is_drawn() {
    // On XZ the pad goes −Y, straight at the front view's eye: 0 points a millimetre.
    let h = pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xz);
    assert!(on_screen(&h, &front([20.0, -10.0, 10.0])).is_empty());
    // A face whose normal leans s off the view's way, (s, −√(1 − s²), 0): 2s points a
    // millimetre. At s 0.31, 0.62: drawn; at 0.29, 0.58: flat, as the prototype's `ppu < 0.6`.
    let leaning = |s: f64| {
        let c = (1.0 - s * s).sqrt();
        SketchPlane::Face(FaceFrame {
            origin: [0.0; 3],
            x: [c, s, 0.0],
            y: [0.0, 0.0, 1.0],
        })
    };
    for (s, drawn) in [(0.31, true), (0.29, false)] {
        let h = pad_handles(&dialog_of(Kind::Pad), leaning(s));
        let a = on_screen(&h, &front(h.distances[0].base));
        assert_eq!(a.distances.len(), usize::from(drawn), "{s}");
        assert_eq!(a.flip.is_some(), drawn, "{s}");
        if drawn {
            assert!((a.distances[0].arrow.per_mm - 2.0 * s).abs() < 1e-5);
        }
    }
    // Zoomed out to 0.5 points a millimetre (200 mm on 200 pixels at 2 a point): flat too,
    // the prototype's threshold being absolute.
    let far = Camera {
        projection: Projection::Orthographic { view_height: 200.0 },
        ..front([20.0, 10.0, 10.0])
    };
    let h = pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xy);
    assert!(on_screen(&h, &far).is_empty());
}

#[test]
fn the_label_writes_the_value_as_the_prototypes_fmt() {
    assert_eq!(label(mm("10")), "10 mm");
    assert_eq!(label(mm("12.5")), "12.5 mm");
    assert_eq!(label(mm("0.5")), "0.5 mm");
    assert_eq!(label(mm("1250")), "1,250 mm");
    // To two decimals, half away from zero, as `toLocaleString` rounds: 12.345 is 12.35.
    assert_eq!(label(mm("12.345")), "12.35 mm");
    assert_eq!(label(mm("12.3")), "12.3 mm");
}

// ---- Hits ----

#[test]
fn a_press_takes_an_arrow_nearer_than_14_points_to_its_tip_or_6_to_its_shaft() {
    // The front view's arrow: base (140, 80), tip (140, 24); the flip arrow (140, 100) to
    // (140, 72).
    let a = on_screen(
        &pad_handles(&dialog_of(Kind::Pad), SketchPlane::Xy),
        &front([20.0, 10.0, 10.0]),
    );
    let length = Some(Hit::Distance(LENGTH));
    for (place, hit) in [
        ((140.0, 24.0), length),
        // Past the tip, square to the shaft: 13.9 is in, 14 isn't (`< 14`).
        ((153.9, 24.0), length),
        ((154.0, 24.0), None),
        // The head's point, 12 past the tip.
        ((140.0, 12.0), length),
        // Beside the shaft, 16 below the tip (and 32 above the flip arrow's tip): 5.9 is
        // in, 6 isn't.
        ((145.9, 40.0), length),
        ((146.0, 40.0), None),
        ((134.1, 40.0), length),
        ((134.0, 40.0), None),
        // Behind the base, along the shaft: 5.9 from it is in.
        ((140.0, 85.9), length),
        // The flip arrow's shaft, 10 from the distance arrow's base.
        ((140.0, 90.0), Some(Hit::Flip)),
        // Where they overlap (5 below the distance arrow's base, on the flip's shaft): the
        // distance arrow's, first.
        ((140.0, 75.0), length),
        ((300.0, 150.0), None),
    ] {
        assert_eq!(a.hit(pos2(place.0, place.1)), hit, "{place:?}");
    }
    assert!(!a.distances[0].arrow.hit(pos2(140.0, 86.0)));
}

// ---- A drag's value ----

/// The 45° view's arrow for `dialog`, √2 points a millimetre up the screen, its tip at
/// (140, 24).
fn tilted_arrow(dialog: &Dialog) -> DistanceArrow {
    let camera = ortho([20.0, 10.0, 10.0], [0.0, -r2(), r2()], [0.0, 0.0, 1.0]);
    on_screen(&pad_handles(dialog, SketchPlane::Xy), &camera)
        .distances
        .remove(0)
}

fn drag_by(arrow: &DistanceArrow, x: f32, y: f32) -> Option<Length> {
    let press = arrow.arrow.tip;
    dragged(
        &arrow.distance,
        &arrow.arrow,
        press,
        press + eframe::egui::vec2(x, y),
    )
}

#[test]
fn a_move_along_the_arrow_moves_the_value_by_its_points_over_the_points_a_millimetre() {
    let arrow = tilted_arrow(&dialog_of(Kind::Pad));
    // Up by 10√2 points: 10 mm, 10 → 20. Down by 5√2: 5.
    assert_eq!(drag_by(&arrow, 0.0, -14.142_136), Some(mm("20")));
    assert_eq!(drag_by(&arrow, 0.0, 7.071_068), Some(mm("5")));
    // To the nearest half millimetre, either side of a quarter (a quarter itself is at the
    // f32 projection's mercy; `round_half_up`'s own test has the half): 0.26 mm up is 10.26,
    // so 10.5; 0.24 is 10.24, so 10; down, 9.76 is 10 and 9.74 is 9.5.
    for (by, to) in [(0.26, "10.5"), (0.24, "10"), (-0.24, "10"), (-0.26, "9.5")] {
        let points = -(by * SQRT_2) as f32;
        assert_eq!(drag_by(&arrow, 0.0, points), Some(mm(to)), "{by}");
    }
    // Square across it: nothing moves (an on-grid value stays).
    assert_eq!(drag_by(&arrow, 100.0, 0.0), Some(mm("10")));
    assert_eq!(drag_by(&arrow, -37.5, 0.0), Some(mm("10")));
    // At an angle: only its part along counts, 10√2 up and 50 across, 10 mm.
    assert_eq!(drag_by(&arrow, 50.0, -14.142_136), Some(mm("20")));
}

#[test]
fn a_value_off_the_half_millimetres_lands_on_them_at_the_first_move_as_the_prototypes_does() {
    // `Math.round((D0 + dd) * 2) / 2` with dd 0: 10.3 is 10.5, 10.2 is 10, 10.25 is 10.5.
    for (from, to) in [
        ("10.3", "10.5"),
        ("10.2", "10"),
        ("10.25", "10.5"),
        ("10.000001", "10"),
    ] {
        let mut dialog = dialog_of(Kind::Pad);
        set_text(&mut dialog, LENGTH, from);
        let arrow = tilted_arrow(&dialog);
        assert_eq!(drag_by(&arrow, 30.0, 0.0), Some(mm(to)), "{from}");
    }
}

#[test]
fn a_drag_past_the_profile_stops_at_half_a_millimetre_and_never_turns_the_sweep_over() {
    let arrow = tilted_arrow(&dialog_of(Kind::Pad));
    // Down 9.75 mm: 0.25, rounded half up to 0.5. Down 9.8: 0.2 → 0, at least 0.5. Down 100:
    // −90 → 0.5.
    assert_eq!(drag_by(&arrow, 0.0, 9.75 * SQRT_2 as f32), Some(mm("0.5")));
    assert_eq!(drag_by(&arrow, 0.0, 9.8 * SQRT_2 as f32), Some(mm("0.5")));
    assert_eq!(drag_by(&arrow, 0.0, 100.0 * SQRT_2 as f32), Some(mm("0.5")));
    // As far down as a point goes: still 0.5.
    assert_eq!(drag_by(&arrow, 0.0, f32::MAX), Some(mm("0.5")));
}

#[test]
fn a_drag_past_what_a_length_holds_gives_nothing_and_a_place_that_isnt_one_nothing() {
    let arrow = tilted_arrow(&dialog_of(Kind::Pad));
    // 1e30 points up: 7e29 mm, past an i64 of nanometres.
    assert_eq!(drag_by(&arrow, 0.0, -1e30), None);
    assert_eq!(drag_by(&arrow, 0.0, -f32::MAX), None);
    assert_eq!(drag_by(&arrow, f32::NAN, 0.0), None);
    assert_eq!(drag_by(&arrow, 0.0, f32::NEG_INFINITY), None);
    // Under what a length holds (9,223,372,036,854 mm): 1.2e13 points up, 8.5e12 mm, a whole
    // number of half millimetres.
    let top = drag_by(&arrow, 0.0, -1.2e13).unwrap();
    // To the projection's f32 rounding of the points a millimetre, a part in 1e7.
    let expected = 10.0 + f64::from(1.2e13_f32) / SQRT_2;
    assert!(
        (top.mm() - expected).abs() <= expected * 1e-7,
        "{top:?} against {expected}"
    );
    assert_eq!(top.nm() % HALF_MM_NM, 0, "{top:?}");
}

#[test]
fn a_symmetric_extents_drag_moves_its_length_by_twice_its_ends_move() {
    let mut dialog = dialog_of(Kind::Pad);
    set_form(&mut dialog, "symmetric");
    // Its end at 5, up 5 mm: the length 10 → 20.
    let arrow = tilted_arrow(&dialog);
    assert_eq!(arrow.distance.factor, 2.0);
    assert_eq!(drag_by(&arrow, 0.0, -5.0 * SQRT_2 as f32), Some(mm("20")));
    // A quarter millimetre up: 10.5 (a half), not 10.25.
    assert_eq!(
        drag_by(&arrow, 0.0, -0.25 * SQRT_2 as f32),
        Some(mm("10.5"))
    );
}

#[test]
fn a_half_rounds_up_as_math_round_rounds_it() {
    // `Math.round`: to the nearest, a half toward +∞.
    for (x, rounded) in [
        (20.5, 21.0),
        (19.5, 20.0),
        (20.49, 20.0),
        (-0.5, 0.0),
        (-1.5, -1.0),
        (-0.51, -1.0),
        (0.0, 0.0),
        // The largest double under a half: below it, so down (`x + 0.5` would round to 1).
        (0.499_999_999_999_999_94, 0.0),
        // 2^52 − ½, the last half a double holds.
        (4_503_599_627_370_495.5, 4_503_599_627_370_496.0),
    ] {
        assert_eq!(round_half_up(x), rounded, "{x}");
    }
}

// ---- Asked of the dialog ----

#[test]
fn a_drags_value_is_written_as_the_document_writes_it_and_only_where_it_differs() {
    let mut dialog = dialog_of(Kind::Pad);
    assert!(apply(&mut dialog, vec![Asked::Drag(LENGTH, mm("12.5"))]));
    assert_eq!(text_of(&dialog, LENGTH), "12.5");
    assert!(apply(&mut dialog, vec![Asked::Drag(LENGTH, mm("1250"))]));
    assert_eq!(text_of(&dialog, LENGTH), "1250");
    // The same length, written another way: left as typed (`v !== doc().p.D`).
    set_text(&mut dialog, LENGTH, "10.0");
    assert!(!apply(&mut dialog, vec![Asked::Drag(LENGTH, mm("10"))]));
    assert_eq!(text_of(&dialog, LENGTH), "10.0");
    // Given back exactly as it was.
    assert!(apply(&mut dialog, vec![Asked::Drag(LENGTH, mm("3"))]));
    assert!(apply(
        &mut dialog,
        vec![Asked::Restore(LENGTH, "10.0".to_owned())]
    ));
    assert_eq!(text_of(&dialog, LENGTH), "10.0");
    assert!(!apply(
        &mut dialog,
        vec![Asked::Restore(LENGTH, "10.0".to_owned())]
    ));
}

#[test]
fn the_flip_turns_reversed_over_and_changes_no_other_field() {
    let mut dialog = dialog_of(Kind::Pad);
    set_text(&mut dialog, LENGTH, "12.5");
    let before = dialog.fields();
    assert!(apply(&mut dialog, vec![Asked::Flip]));
    assert!(reversed(&dialog));
    let after = dialog.fields();
    // The extent alone, as `set_field` sends it whole: its reversed alone changed.
    for (field, text) in &before {
        if *field != "extent" {
            assert_eq!(after.get(field), Some(text), "{field}");
        }
    }
    assert_eq!(after.len(), before.len());
    assert_eq!(
        before.get("extent").cloned().flatten().as_deref(),
        Some(r#"{"type": "one_side", "length": 12.5, "reversed": false}"#)
    );
    assert_eq!(
        after.get("extent").cloned().flatten().as_deref(),
        Some(r#"{"type": "one_side", "length": 12.5, "reversed": true}"#)
    );
    assert_eq!(text_of(&dialog, LENGTH), "12.5");
    assert!(apply(&mut dialog, vec![Asked::Flip]));
    assert!(!reversed(&dialog));
    // A dialog with no Reversed: nothing.
    let mut fillet = Dialog::new(Kind::Fillet, None, 1);
    assert!(!apply(&mut fillet, vec![Asked::Flip]));
}

// ---- A frame's events ----

fn event(kind: LeftKind, at: (f32, f32), seq: u32) -> LeftEvent {
    LeftEvent {
        kind,
        at_px: [(at.0 - ORIGIN.x) * PPP, (at.1 - ORIGIN.y) * PPP],
        shift: false,
        ctrl: false,
        frame: 0,
        time: 0.0,
        seq,
    }
}

fn front_arrows(dialog: &Dialog) -> Arrows {
    on_screen(
        &pad_handles(dialog, SketchPlane::Xy),
        &front([20.0, 10.0, 10.0]),
    )
}

#[test]
fn a_press_on_the_arrow_takes_its_stream_and_each_move_drags_it() {
    let dialog = dialog_of(Kind::Pad);
    let arrows = front_arrows(&dialog);
    let mut grip = Grip::default();
    // On the tip, up 20 points at 2 a millimetre (10 mm), then 3 more (11.5): each its value.
    let events = [
        event(LeftKind::Press, (140.0, 24.0), 0),
        event(LeftKind::Move, (140.0, 4.0), 1),
        event(LeftKind::Move, (141.0, 1.0), 2),
    ];
    let (passed, asked) = grip.frame(&events, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    assert!(passed.is_empty());
    assert_eq!(
        asked,
        [
            Asked::Drag(LENGTH, mm("20")),
            Asked::Drag(LENGTH, mm("21.5"))
        ]
    );
    assert!(grip.holding());
    assert_eq!(grip.held(), Some(Hit::Distance(LENGTH)));
    assert_eq!(grip.press_px(), Some([200.0, -12.0]));
    // Let go: taken, asking nothing more.
    let (passed, asked) = grip.frame(
        &[event(LeftKind::Release, (141.0, 1.0), 0)],
        Some(&arrows),
        Some(&dialog),
        PPP,
        ORIGIN,
    );
    assert!(passed.is_empty() && asked.is_empty());
    assert!(!grip.holding());
    assert_eq!(grip.press_px(), None);
}

#[test]
fn a_stream_pressed_off_the_arrows_is_given_back_whole() {
    let dialog = dialog_of(Kind::Pad);
    let arrows = front_arrows(&dialog);
    let mut grip = Grip::default();
    let events = [
        event(LeftKind::Press, (300.0, 150.0), 0),
        // Over the arrow mid-stream: still not the arrow's.
        event(LeftKind::Move, (140.0, 24.0), 1),
        event(LeftKind::Release, (140.0, 24.0), 2),
        event(LeftKind::DoubleClick, (140.0, 24.0), 3),
    ];
    let (passed, asked) = grip.frame(&events, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    assert_eq!(passed, events);
    assert!(asked.is_empty());
    // With no dialog, a press on where the arrow was is given back too.
    let press = [event(LeftKind::Press, (140.0, 24.0), 0)];
    let (passed, _) = grip.frame(&press, Some(&arrows), None, PPP, ORIGIN);
    assert_eq!(passed, press);
}

#[test]
fn a_cancelled_drag_gives_its_input_back_the_text_it_held_at_the_press() {
    let mut dialog = dialog_of(Kind::Pad);
    set_text(&mut dialog, LENGTH, "10.0");
    let arrows = front_arrows(&dialog);
    let mut grip = Grip::default();
    let events = [
        event(LeftKind::Press, (140.0, 24.0), 0),
        event(LeftKind::Move, (140.0, 4.0), 1),
        event(LeftKind::Cancel, (140.0, 4.0), 2),
    ];
    let (passed, asked) = grip.frame(&events, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    assert!(passed.is_empty());
    assert_eq!(
        asked,
        [
            Asked::Drag(LENGTH, mm("20")),
            Asked::Restore(LENGTH, "10.0".to_owned())
        ]
    );
    assert!(!grip.holding());
}

#[test]
fn esc_ends_a_drag_where_it_began_and_the_rest_of_its_stream_does_nothing() {
    let dialog = dialog_of(Kind::Pad);
    let arrows = front_arrows(&dialog);
    let mut grip = Grip::default();
    let press = [
        event(LeftKind::Press, (140.0, 24.0), 0),
        event(LeftKind::Move, (140.0, 4.0), 1),
    ];
    grip.frame(&press, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    // Another dialog's Esc takes nothing.
    assert_eq!(grip.escape(Some(8)), None);
    assert_eq!(
        grip.escape(Some(7)),
        Some(vec![Asked::Restore(LENGTH, "10".to_owned())])
    );
    assert_eq!(grip.held(), None);
    // A second Esc goes on down the chain.
    assert_eq!(grip.escape(Some(7)), None);
    let rest = [
        event(LeftKind::Move, (140.0, -20.0), 0),
        event(LeftKind::Release, (140.0, -20.0), 1),
        event(LeftKind::DoubleClick, (140.0, -20.0), 2),
    ];
    let (passed, asked) = grip.frame(&rest, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    assert!(
        passed.is_empty() && asked.is_empty(),
        "{passed:?} {asked:?}"
    );
    assert!(!grip.holding());
}

#[test]
fn a_dialog_closed_mid_drag_leaves_the_rest_of_the_stream_taken_and_asking_nothing() {
    let dialog = dialog_of(Kind::Pad);
    let arrows = front_arrows(&dialog);
    let mut grip = Grip::default();
    grip.frame(
        &[event(LeftKind::Press, (140.0, 24.0), 0)],
        Some(&arrows),
        Some(&dialog),
        PPP,
        ORIGIN,
    );
    // Closed: no dialog, no arrows.
    let rest = [
        event(LeftKind::Move, (140.0, 4.0), 0),
        event(LeftKind::Cancel, (140.0, 4.0), 1),
    ];
    let (passed, asked) = grip.frame(&rest, None, None, PPP, ORIGIN);
    assert!(
        passed.is_empty() && asked.is_empty(),
        "{passed:?} {asked:?}"
    );
    // Nor does Esc go to it.
    assert_eq!(grip.escape(None), None);
    // Another dialog opened in its place, mid-stream: still nothing.
    grip.frame(
        &[event(LeftKind::Press, (140.0, 24.0), 0)],
        Some(&arrows),
        Some(&dialog),
        PPP,
        ORIGIN,
    );
    let other = Dialog::new(Kind::Pad, Some(FeatureId::new("s0000001").unwrap()), 8);
    let (passed, asked) = grip.frame(
        &[event(LeftKind::Move, (140.0, 4.0), 0)],
        Some(&arrows),
        Some(&other),
        PPP,
        ORIGIN,
    );
    assert!(passed.is_empty() && asked.is_empty());
    assert_eq!(grip.held(), None);
    assert_eq!(grip.escape(Some(8)), None);
}

#[test]
fn a_click_on_the_flip_arrow_flips_and_a_press_let_go_off_it_doesnt() {
    let dialog = dialog_of(Kind::Pad);
    let arrows = front_arrows(&dialog);
    let mut grip = Grip::default();
    let click = [
        event(LeftKind::Press, (140.0, 90.0), 0),
        event(LeftKind::Release, (140.0, 90.0), 1),
        event(LeftKind::DoubleClick, (140.0, 90.0), 2),
    ];
    let (passed, asked) = grip.frame(&click, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    // Its double-click the arrows' too.
    assert!(passed.is_empty());
    assert_eq!(asked, [Asked::Flip]);
    // Moved off and back on before the release: a flip.
    let back = [
        event(LeftKind::Press, (140.0, 90.0), 0),
        event(LeftKind::Move, (300.0, 150.0), 1),
        event(LeftKind::Move, (141.0, 92.0), 2),
        event(LeftKind::Release, (141.0, 92.0), 3),
    ];
    let (_, asked) = grip.frame(&back, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    assert_eq!(asked, [Asked::Flip]);
    // Let go off it: nothing, and its stream all taken.
    let off = [
        event(LeftKind::Press, (140.0, 90.0), 0),
        event(LeftKind::Move, (300.0, 150.0), 1),
        event(LeftKind::Release, (300.0, 150.0), 2),
    ];
    let (passed, asked) = grip.frame(&off, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    assert!(passed.is_empty() && asked.is_empty());
    // Cancelled, or Esc'd, before the release: nothing.
    let cancelled = [
        event(LeftKind::Press, (140.0, 90.0), 0),
        event(LeftKind::Cancel, (140.0, 90.0), 1),
    ];
    let (_, asked) = grip.frame(&cancelled, Some(&arrows), Some(&dialog), PPP, ORIGIN);
    assert!(asked.is_empty());
    grip.frame(
        &[event(LeftKind::Press, (140.0, 90.0), 0)],
        Some(&arrows),
        Some(&dialog),
        PPP,
        ORIGIN,
    );
    assert_eq!(grip.held(), Some(Hit::Flip));
    assert_eq!(grip.escape(Some(7)), Some(Vec::new()));
    let (_, asked) = grip.frame(
        &[event(LeftKind::Release, (140.0, 90.0), 0)],
        Some(&arrows),
        Some(&dialog),
        PPP,
        ORIGIN,
    );
    assert!(asked.is_empty());
}

#[test]
fn the_drag_keeps_the_arrows_way_and_scale_from_its_press_whatever_the_arrows_do_after() {
    let dialog = dialog_of(Kind::Pad);
    let mut grip = Grip::default();
    grip.frame(
        &[event(LeftKind::Press, (140.0, 24.0), 0)],
        Some(&front_arrows(&dialog)),
        Some(&dialog),
        PPP,
        ORIGIN,
    );
    // The view turned mid-drag (none of its arrows where the press was): the press's
    // 2 points a millimetre up the screen still measure the move.
    let (_, asked) = grip.frame(
        &[event(LeftKind::Move, (140.0, 14.0), 0)],
        Some(&Arrows::default()),
        Some(&dialog),
        PPP,
        ORIGIN,
    );
    assert_eq!(asked, [Asked::Drag(LENGTH, mm("15"))]);
}

#[test]
fn a_profiles_centroid_is_worked_out_once_a_read() {
    let mut grip = Grip::default();
    let read = Arc::new(rectangle_read());
    assert_eq!(grip.centroid(&read), Some([20.0, 10.0]));
    // Another read, equal or not, is worked out again; the same one is kept.
    let mut moved = rectangle_read();
    for p in &mut moved.members.points {
        p.x = Coordinate::new(p.x.mm() + 10.0).unwrap();
    }
    let moved = Arc::new(moved);
    assert_eq!(grip.centroid(&moved), Some([30.0, 10.0]));
    assert_eq!(grip.centroid(&moved), Some([30.0, 10.0]));
    assert_eq!(grip.centroid(&Arc::new(under(rectangle(Vec::new())))), None);
}

// ---- The shapes ----

#[test]
fn its_shapes_are_the_prototypes_in_its_order_and_the_held_one_white() {
    let dialog = dialog_of(Kind::Pad);
    let arrows = front_arrows(&dialog);
    let blue = Color32::from_rgb(0x46, 0x76, 0xff);
    let white = Color32::from_rgb(0xff, 0xff, 0xff);
    for (held, flip_colour, colour) in [
        (None, blue, blue),
        (Some(Hit::Distance(LENGTH)), blue, white),
        (Some(Hit::Flip), white, blue),
    ] {
        with_painter(|painter| {
            let shapes = arrows.shapes(painter, held);
            assert_eq!(shapes.len(), 8, "{shapes:?}");
            // The flip arrow: its shaft (140, 100) to (140, 72), 2.5 wide; its head.
            let Shape::LineSegment { points, stroke } = &shapes[0] else {
                panic!("{:?}", shapes[0]);
            };
            at(points[0], [140.0, 100.0]);
            at(points[1], [140.0, 72.0]);
            assert_eq!((stroke.width, stroke.color), (2.5, flip_colour));
            let Shape::Path(head) = &shapes[1] else {
                panic!("{:?}", shapes[1]);
            };
            assert_eq!((head.closed, head.fill), (true, flip_colour));
            assert_eq!(head.points.len(), 3);
            at(head.points[0], [140.0, 60.0]);
            // The distance arrow: shaft, head, the dot over its base.
            let Shape::LineSegment { points, stroke } = &shapes[2] else {
                panic!("{:?}", shapes[2]);
            };
            at(points[0], [140.0, 80.0]);
            at(points[1], [140.0, 24.0]);
            assert_eq!((stroke.width, stroke.color), (2.5, colour));
            let Shape::Path(head) = &shapes[3] else {
                panic!("{:?}", shapes[3]);
            };
            at(head.points[0], [140.0, 12.0]);
            at(head.points[1], [146.0, 24.0]);
            at(head.points[2], [134.0, 24.0]);
            assert_eq!(head.fill, colour);
            let Shape::Circle(dot) = &shapes[4] else {
                panic!("{:?}", shapes[4]);
            };
            at(dot.center, [140.0, 80.0]);
            assert_eq!((dot.radius, dot.fill), (3.5, colour));
            // The label: its box from (146, 2 − 10), 20 high, its words' width and 6 each
            // side, filled #212121, outlined 1 point in the arrow's colour; "10 mm" in white,
            // 6 in.
            let words = painter.layout_no_wrap(
                "10 mm".to_owned(),
                eframe::egui::FontId::proportional(11.5),
                white,
            );
            let Shape::Rect(fill) = &shapes[5] else {
                panic!("{:?}", shapes[5]);
            };
            at(fill.rect.min, [146.0, -8.0]);
            assert!((fill.rect.height() - 20.0).abs() < 1e-4);
            assert!((fill.rect.width() - (words.size().x + 12.0)).abs() < 1e-4);
            assert_eq!(fill.fill, Color32::from_rgb(0x21, 0x21, 0x21));
            let Shape::Rect(outline) = &shapes[6] else {
                panic!("{:?}", shapes[6]);
            };
            assert_eq!(outline.rect, fill.rect);
            assert_eq!((outline.stroke.width, outline.stroke.color), (1.0, colour));
            let Shape::Text(text) = &shapes[7] else {
                panic!("{:?}", shapes[7]);
            };
            assert_eq!(text.galley.text(), "10 mm");
            assert!((text.pos.x - 152.0).abs() < 1e-4, "{:?}", text.pos);
            assert_eq!(text.fallback_color, white);
        });
    }
}

#[test]
fn the_cursor_is_grab_over_a_distance_arrow_and_a_hand_over_the_flip() {
    assert_eq!(cursor(Hit::Distance(LENGTH)), CursorIcon::Grab);
    assert_eq!(cursor(Hit::Flip), CursorIcon::PointingHand);
}
