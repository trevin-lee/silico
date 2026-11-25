//! M6.3 on the real kernel: linear patterns, polar patterns, and mirrors rebuilt
//! (docs/design/m6.0-patterns.md, sections 2.4, 4.2, and 3; docs/ports/patterns.md §5 and
//! §7). FreeCAD's tests, their seeds sketched and padded (primitives left the app); volumes
//! and positions worked by hand; each refusal of section 4.2 whose verdict is OCCT's, held
//! to the design's example byte for byte; the verdicts each step's history gives; and copies'
//! names as stored references follow them. Sketches are solved as stored, by the fake
//! sketch solver.

use super::close::assert_close;
use super::pattern_docs::{
    self, SOLID, THROUGH, direction, down, failure, linear_as, mirror, of, polar, summary, up, with,
};
use super::sweeps::{circles, id, pad, pocket, polygon, rectangle};

use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use silico_core::command::wire::{Layout, ToWire, write as wire};
use silico_core::document::{Checked, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::{KernelRequest, KernelWorker};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use silico_protocol::{EdgeCurve, PointPlace};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// Long enough for any rebuild here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(300);

fn engine() -> Recompute<KernelPool> {
    let pool = KernelPool::start_workers(Path::new(WORKER), 2).unwrap();
    Recompute::new(pool).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<KernelPool>, features: &[String]) -> Arc<Model> {
    pattern_docs::published(engine, features, HANG)
}

fn built(features: &[String]) -> Arc<Model> {
    published(&engine(), features)
}

/// A `side` mm cube from the origin: a square sketched on xy, padded `side`.
fn cube(side: f64) -> Vec<String> {
    vec![
        rectangle("s0000001", "xy", [0.0, 0.0], side, side),
        pad("p0000001", None, "s0000001", &up(&format!("{side}"))),
    ]
}

/// Section 4.2's examples, in order.
fn rebuild_examples() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m6.0-patterns.md");
    let doc = std::fs::read_to_string(path).unwrap();
    let start = doc.find("### 4.2 The rebuild").unwrap();
    let end = start + doc[start..].find("### 4.3 The warning").unwrap();
    doc[start..end]
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

fn written(error: &Error) -> String {
    wire(&error.to_wire(), Layout::Canonical).unwrap()
}

/// The area two circles of radius `r`, `d` apart, share: 2r² acos(d / 2r) − (d / 2)√(4r² − d²).
fn lens(r: f64, d: f64) -> f64 {
    2.0 * r * r * libm::acos(d / (2.0 * r)) - d / 2.0 * libm::sqrt(4.0 * r * r - d * d)
}

#[test]
fn freecads_linear_pattern_ten_cubes_along_each_axis_make_one_solid_of_10000() {
    // TestLinearPattern.py's testXAxisLinearPattern and its Y and Z kin: a 10 mm cube, 10
    // occurrences over 90, touching face to face: 10,000, one solid, unrefined 42 faces, 84
    // edges, 44 vertices (FreeCAD 1.1.3, Refine off: patterns.md §5).
    for axis in ["x", "y", "z"] {
        let model = built(&with(
            cube(10.0),
            &[linear_as(
                "t0000001",
                "p0000001",
                &of("p0000001"),
                &[direction(axis, "extent", "90", 10)],
                "[]",
            )],
        ));
        let s = summary(&model, "t0000001");
        assert_close(axis, s.volume, 10_000.0, 1e-9);
        assert_eq!((s.faces, s.edges, s.vertices), (42, 84, 44), "{axis}");
    }
}

#[test]
fn freecads_polar_pattern_four_cubes_turned_about_an_axis_make_one_solid_of_4000() {
    // TestPolarPattern.py: a 10 mm cube from the origin turned 4 times over 360° about an
    // origin axis: the four meet at it, 4,000, one solid, unrefined 16, 32, 18 (1.1.3).
    for axis in ["z", "x", "y"] {
        let model = built(&with(
            cube(10.0),
            &[polar(
                "t0000001",
                "p0000001",
                &of("p0000001"),
                axis,
                "360",
                4,
            )],
        ));
        let s = summary(&model, "t0000001");
        assert_close(axis, s.volume, 4000.0, 1e-9);
        assert_eq!((s.faces, s.edges, s.vertices), (16, 32, 18), "{axis}");
    }
}

#[test]
fn freecads_mirrored_cubes_double_and_its_offset_case_is_refused_off_the_base() {
    // TestMirrored.py: a 10 mm cube about xy, 2,000, 10, 20, 12 (1.1.3); a unit cube, 2.
    let model = built(&with(
        cube(10.0),
        &[mirror("m0000001", "p0000001", &of("p0000001"), "xy")],
    ));
    let s = summary(&model, "m0000001");
    assert_close("10 mm cube", s.volume, 2000.0, 1e-9);
    assert_eq!((s.faces, s.edges, s.vertices), (10, 20, 12));
    let model = built(&with(
        cube(1.0),
        &[mirror("m0000001", "p0000001", SOLID, "xy")],
    ));
    assert_close("unit cube", summary(&model, "m0000001").volume, 2.0, 1e-9);
    // testMirroredOffsetCase: a unit square at (0, 1)–(1, 2), padded 1, mirrored about the
    // sketch's H axis (the xz plane): the copy lies 2 mm off, which FreeCAD keeps as a
    // compound, and Silico refuses as off the base (decision 6).
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 1.0], 1.0, 1.0),
        pad("p0000001", None, "s0000001", &up("1")),
        mirror("m0000001", "p0000001", &of("p0000001"), "xz"),
    ]);
    let error = failure(&model, "m0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(
        error.message(),
        "mirror m0000001 makes 2 solids: the copy of instance 1 doesn't touch the base, or meets it only along an edge or at a point"
    );
    assert_eq!(error.elements(), ["1"]);
}

#[test]
fn a_copy_off_the_base_of_one_of_several_originals_is_named_with_its_original() {
    // Section 4.2, decision 6: a pattern of two originals, a boss and a hole, on a 20 mm
    // block 5 thick. The boss (r 1 at (5, 5), 8 tall) copied 30 along y stands at (5, 35),
    // clear of the block: its step's result is two solids, its base's faces in one, and the
    // refusal names whose copy it is, as one original's never does (above).
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 20.0, 20.0),
        pad("p0000001", None, "s0000001", &up("5")),
        circles("s0000002", "xy", &[([5.0, 5.0], 1.0)]),
        pad("p0000002", Some("p0000001"), "s0000002", &up("8")),
        circles("s0000003", "xy", &[([15.0, 5.0], 1.0)]),
        pocket("q0000001", "p0000002", "s0000003", THROUGH),
        linear_as(
            "t0000001",
            "q0000001",
            r#"{"type": "features", "features": ["q0000001", "p0000002"]}"#,
            &[direction("y", "spacing", "30", 2)],
            "[]",
        ),
    ]);
    let error = failure(&model, "t0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(
        error.message(),
        "linear_pattern t0000001 makes 2 solids: the copy of p0000002's instance 1 doesn't touch the base, or meets it only along an edge or at a point"
    );
    assert_eq!(error.elements(), ["1"]);
}

#[test]
fn mains_suppression_grid_keeps_a_skip_past_a_count_and_skips_it_again_once_reached() {
    // FreeCAD main's testSuppressionKeepsGridPosition, testSuppressionPersistence, and
    // testSuppressedIndicesOnlyProjectPositionsInsideTheGrid (patterns.md §5): 3 × 3 10 mm
    // cubes 10 apart, (1, 2) skipped: 8,000; direction 2's count 4, the skip still (1, 2):
    // 11,000, the skipped cube's (15, 25, 5) outside and (1, 1)'s (15, 15, 5) inside; the
    // counts 1 and 2, the skip past them kept and taking nothing: 2,000.
    let grid = |first: u32, second: u32| {
        with(
            cube(10.0),
            &[linear_as(
                "t0000001",
                "p0000001",
                &of("p0000001"),
                &[
                    direction("x", "spacing", "10", first),
                    direction("y", "spacing", "10", second),
                ],
                "[[1, 2]]",
            )],
        )
    };
    let engine = engine();
    assert_close(
        "3 × 3",
        summary(&published(&engine, &grid(3, 3)), "t0000001").volume,
        8000.0,
        1e-9,
    );
    let four = published(&engine, &grid(3, 4));
    assert_close("3 × 4", summary(&four, "t0000001").volume, 11_000.0, 1e-9);
    let Some(FeatureOutcome::Built { built, .. }) = four.outcome(&id("t0000001")) else {
        panic!("not built");
    };
    let mut worker = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    for (point, place) in [
        ([15.0, 25.0, 5.0], PointPlace::Outside),
        ([15.0, 15.0, 5.0], PointPlace::Inside),
    ] {
        assert_eq!(
            worker
                .run(KernelRequest::classify(&built.shape, point))
                .unwrap(),
            place,
            "{point:?}"
        );
    }
    assert_close(
        "1 × 2",
        summary(&published(&engine, &grid(1, 2)), "t0000001").volume,
        2000.0,
        1e-9,
    );
    // Through a save: direction 2's count 1, saved and read back, then 4: 11,000 again.
    let one = published(&engine, &grid(3, 1));
    let text = silico_core::document::write(&one.saved()).unwrap();
    assert!(text.contains("[1, 2]"), "{text}");
    let raised = text.replacen("\"count\": 1\n", "\"count\": 4\n", 1);
    let document = Checked::new(read(raised.as_bytes()).unwrap()).unwrap();
    let Ok(Ok(RunOutcome::Published(model))) = engine.start(document).wait_timeout(HANG) else {
        panic!("not published");
    };
    assert_close(
        "saved, then 3 × 4",
        summary(&model, "t0000001").volume,
        11_000.0,
        1e-9,
    );
}

#[test]
fn three_cylinders_over_40_on_a_box_add_135_pi() {
    // patterns.md §7: three cylinders r 3, 5 tall above a 100 × 20 × 10 box, over 40:
    // 20,000 + 3 · π · 9 · 5 = 20,424.115008. Each is padded 15 from xy, 10 of it inside.
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 100.0, 20.0),
        pad("p0000001", None, "s0000001", &up("10")),
        circles("s0000002", "xy", &[([10.0, 10.0], 3.0)]),
        pad("p0000002", Some("p0000001"), "s0000002", &up("15")),
        linear_as(
            "t0000001",
            "p0000002",
            &of("p0000002"),
            &[direction("x", "extent", "40", 3)],
            "[]",
        ),
    ]);
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, 20_000.0 + 135.0 * PI, 1e-9);
    // The last cylinder's centre at x = 50: the box's top reaches x = 100, the tops 53.
    assert_close("top", s.bounds.unwrap()[1][2], 15.0, 1e-12);
}

#[test]
fn three_overlapping_holes_remove_the_union_of_their_circles() {
    // patterns.md §2: three holes r 3, 2 apart, through a 10 mm box, each a separate tool in
    // one cut: the union of three circles in a row, 27π − 2 · lens(3, 2) (the outer two's
    // lens lies in the middle one), times 10: 518.221711.
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 100.0, 20.0),
        pad("p0000001", None, "s0000001", &up("10")),
        circles("s0000002", "xy", &[([20.0, 10.0], 3.0)]),
        pocket("q0000001", "p0000001", "s0000002", THROUGH),
        linear_as(
            "t0000001",
            "q0000001",
            &of("q0000001"),
            &[direction("x", "spacing", "2", 3)],
            "[]",
        ),
    ]);
    let removed = 20_000.0 - summary(&model, "t0000001").volume;
    let union = 27.0 * PI - 2.0 * lens(3.0, 2.0);
    assert_close("removed", removed, 10.0 * union, 1e-7);
    assert_close("by hand", 10.0 * union, 518.221711, 1e-9);
}

#[test]
fn a_mirror_1_mm_off_a_cylinders_axis_adds_two_cylinders_less_their_lens() {
    // patterns.md §1: a cylinder r 3, 5 tall above the box, its axis 1 mm from the mirror
    // plane (yz): the two overlap, 5 · (18π − lens(3, 2)) = 200.241263 above the box's
    // 20,000.
    let model = built(&[
        rectangle("s0000001", "xy", [-50.0, 0.0], 100.0, 20.0),
        pad("p0000001", None, "s0000001", &up("10")),
        circles("s0000002", "xy", &[([1.0, 10.0], 3.0)]),
        pad("p0000002", Some("p0000001"), "s0000002", &up("15")),
        mirror("m0000001", "p0000002", &of("p0000002"), "yz"),
    ]);
    let want = 20_000.0 + 5.0 * (18.0 * PI - lens(3.0, 2.0));
    assert_close("volume", summary(&model, "m0000001").volume, want, 1e-9);
    assert_close("by hand", want, 20_200.241263, 1e-9);
}

#[test]
fn a_boxs_bottom_face_as_a_direction_patterns_down_as_its_outward_normal_points() {
    // Decision 3: a face points along its outward normal, as describe reports it. The pad's
    // bottom face, `M` of its sketch's face, points down: the whole block copied 10 along it
    // sits under it, touching, from z = −10.
    let model = built(&with(
        vec![
            rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0),
            pad("p0000001", None, "s0000001", &up("10")),
        ],
        &[linear_as(
            "t0000001",
            "p0000001",
            SOLID,
            &[direction(
                "p0000001.pad.Face(M s0000001.sketch.Face1)",
                "spacing",
                "10",
                2,
            )],
            "[]",
        )],
    ));
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, 16_000.0, 1e-9);
    let [low, high] = s.bounds.unwrap();
    assert_close("bottom", low[2], -10.0, 1e-12);
    assert_close("top", high[2], 10.0, 1e-12);
}

#[test]
fn copies_of_a_pad_off_a_block_are_refused_naming_exactly_their_instances() {
    // Section 4.2's example: a boss r 3 on a 40 mm block's top at x = 5, patterned along x
    // 13 apart, 5 times: copies 3 and 4, centred at 44 and 57, miss the block.
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0),
        pad("p0000001", None, "s0000001", &up("10")),
        circles("s0000002", "xy", &[([5.0, 10.0], 3.0)]),
        pad("p0000002", Some("p0000001"), "s0000002", &up("15")),
        linear_as(
            "t0000005",
            "p0000002",
            &of("p0000002"),
            &[direction("x", "spacing", "13", 5)],
            "[]",
        ),
    ]);
    assert_eq!(written(&failure(&model, "t0000005")), rebuild_examples()[5]);
    let verdicts = model.pattern_verdicts(&id("t0000005")).unwrap();
    let off: Vec<String> = verdicts[0]
        .off_base
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(off, ["3", "4"]);
}

#[test]
fn slots_cutting_a_disc_in_three_are_refused_naming_no_instance() {
    // Section 4.2's example: a slot from a disc's rim past its centre, turned three times
    // about its axis: the centre is cut out, and the disc comes apart in three.
    let model = built(&[
        circles("s0000001", "xy", &[([0.0, 0.0], 20.0)]),
        pad("p0000001", None, "s0000001", &up("5")),
        rectangle("s0000002", "xy", [-1.0, -2.0], 2.0, 27.0),
        pocket("q0000006", "p0000001", "s0000002", THROUGH),
        polar("t0000006", "q0000006", &of("q0000006"), "z", "360", 3),
    ]);
    assert_eq!(written(&failure(&model, "t0000006")), rebuild_examples()[6]);
}

#[test]
fn a_far_copy_that_leaves_its_base_in_pieces_is_refused_with_the_fuzzy_value_its_cause() {
    // Section 4.2's example (open decision 4(a)): a pin r 2 on a 20 × 20 plate, copied
    // 0.001 along x and a kilometre along y: the boolean's box is a kilometre across, its
    // fuzzy value about a millimetre, and the pin parts from its plate (patterns.md, *Open
    // questions*: the plate's faces and the pins' in solids apart).
    let model = built(&[
        rectangle("s0000001", "xy", [-10.0, -10.0], 20.0, 20.0),
        pad("p0000011", None, "s0000001", &down("1")),
        circles("s0000002", "xy", &[([0.0, 0.0], 2.0)]),
        pad("p0000012", Some("p0000011"), "s0000002", &up("4")),
        linear_as(
            "t0000007",
            "p0000012",
            &of("p0000012"),
            &[
                direction("x", "spacing", "0.001", 2),
                direction("y", "spacing", "1000000", 2),
            ],
            "[]",
        ),
    ]);
    assert_eq!(written(&failure(&model, "t0000007")), rebuild_examples()[7]);
    let verdict = &model.pattern_verdicts(&id("t0000007")).unwrap()[0];
    assert_eq!((verdict.solids, verdict.base_solids), (3, 2));
    assert!(verdict.off_base.is_empty());
}

#[test]
fn a_mirror_about_its_originals_own_plane_of_symmetry_is_refused_by_volume() {
    // Section 4.2's example: the plate's notch, symmetric about xz, mirrored about it: the
    // copy lands on the original, and the result's volume is the base's.
    let model = built(&[
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", &up("5")),
        rectangle("s0000002", "xy", [44.0, -5.0], 6.0, 10.0),
        pocket("q0000001", "p0000001", "s0000002", THROUGH),
        mirror("m0000001", "q0000001", &of("q0000001"), "xz"),
    ]);
    assert_eq!(written(&failure(&model, "m0000001")), rebuild_examples()[8]);
}

#[test]
fn twenty_holes_on_a_175_plate_cut_17_and_keep_the_three_that_miss_as_changed_nothing() {
    // patterns.md §7, decision 6: twenty holes r 2.75, 10 apart, the original 10 from the
    // plate's end: copies 17, 18, and 19 (at 180, 190, 200) lie past the 175 mm plate, and
    // 17 holes are cut, exactly as asked; the step's history names the three.
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 175.0, 20.0),
        pad("p0000001", None, "s0000001", &up("5")),
        circles("s0000002", "xy", &[([10.0, 10.0], 2.75)]),
        pocket("q0000009", "p0000001", "s0000002", THROUGH),
        linear_as(
            "t0000009",
            "q0000009",
            &of("q0000009"),
            &[direction("x", "spacing", "10", 20)],
            "[]",
        ),
    ]);
    let hole = PI * 2.75 * 2.75 * 5.0;
    assert_close(
        "volume",
        summary(&model, "t0000009").volume,
        175.0 * 20.0 * 5.0 - 17.0 * hole,
        1e-9,
    );
    let verdicts = model.pattern_verdicts(&id("t0000009")).unwrap();
    let nothing: Vec<String> = verdicts[0]
        .changed_nothing
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(nothing, ["17", "18", "19"]);
    assert!(verdicts[0].off_base.is_empty());
    // The warning, section 4.3's example byte for byte, the only one.
    // Every feature spared its fingerprint's warnings (the documents store none), which
    // the pattern's isn't.
    let spared = model
        .document()
        .features
        .iter()
        .map(|f| f.id.clone())
        .collect();
    let warnings = silico_core::warnings::of(&model, &spared);
    let written: Vec<String> = warnings
        .listed()
        .iter()
        .map(|warning| wire(&warning.to_wire(), Layout::Canonical).unwrap())
        .collect();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m6.0-patterns.md");
    let doc = std::fs::read_to_string(path).unwrap();
    let start = doc.find("### 4.3 The warning").unwrap();
    let example = doc[start..]
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    assert_eq!(written, [example]);
}

#[test]
fn more_than_100_copies_that_change_nothing_are_each_kept() {
    // Section 4.2, *Every verdict kept*: copy 1 cuts its hole, and copies 2 to 120 lie past
    // the plate, each kept, though a warning lists 100.
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 10.0),
        pad("p0000001", None, "s0000001", &up("5")),
        circles("s0000002", "xy", &[([5.0, 5.0], 2.75)]),
        pocket("q0000001", "p0000001", "s0000002", THROUGH),
        linear_as(
            "t0000001",
            "q0000001",
            &of("q0000001"),
            &[direction("x", "spacing", "30", 121)],
            "[]",
        ),
    ]);
    let verdicts = model.pattern_verdicts(&id("t0000001")).unwrap();
    let nothing: Vec<String> = verdicts[0]
        .changed_nothing
        .iter()
        .map(ToString::to_string)
        .collect();
    let want: Vec<String> = (2..=120).map(|k: u32| k.to_string()).collect();
    assert_eq!(nothing, want);
    // The warning lists the first 100, and counts all 119.
    // Every feature spared its fingerprint's warnings (the documents store none), which
    // the pattern's isn't.
    let spared = model
        .document()
        .features
        .iter()
        .map(|f| f.id.clone())
        .collect();
    let warnings = silico_core::warnings::of(&model, &spared);
    let [warning] = warnings.listed() else {
        panic!("{:?}", warnings.listed());
    };
    assert_eq!(warning.elements(), &want[..100]);
    assert_eq!(warning.elements_total(), 119);
}

/// The plate of section 2.2 to its corner hole (r 2.75 at (−40, −20)), and a linear pattern
/// of it, `t0000001`, of `directions`, and a chamfer of 0.5 on each of `edges`.
fn plate(directions: &[String], edges: &[String]) -> Vec<String> {
    let mut features = vec![
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", &up("5")),
        circles("s0000003", "xy", &[([-40.0, -20.0], 2.75)]),
        pocket("q0000002", "p0000001", "s0000003", THROUGH),
        linear_as("t0000001", "q0000002", &of("q0000002"), directions, "[]"),
    ];
    for (i, edge) in edges.iter().enumerate() {
        let base = if i == 0 {
            "t0000001".to_owned()
        } else {
            format!("c000000{i}")
        };
        features.push(format!(
            r#"{{"id": "c000000{}", "type": "chamfer", "v": 1, "name": "Rim", "base": "{base}", "edges": ["{edge}"], "size": {{"distance": 0.5}}}}"#,
            i + 1
        ));
    }
    features
}

/// Copy `instance`'s top rim, as section 3 names copy (2, 1)'s: where its wall meets the
/// plate's top.
fn rim(instance: &str) -> String {
    format!(
        "t0000001.linear_pattern.Edge(G p0000001.pad.Face(G s0000001.sketch.Face1), G t0000001.linear_pattern.Face(C{instance} q0000002.pocket.Face(G s0000003.sketch.Edge.ca000001)))"
    )
}

/// What a chamfer of 0.5 on a hole r 2.75's rim removes: a ring of a right triangle of legs
/// 0.5, about the hole's axis, π · 0.5² · (2.75 + 0.5 / 3) (section 2.2).
fn chamfered_away() -> f64 {
    PI * 0.25 * (2.75 + 0.5 / 3.0)
}

/// The centre of the circle feature `feature`'s solid's edge `name` is.
fn rim_centre(model: &Model, feature: &str, name: &str) -> [f64; 3] {
    let map = model.element_map(&id(feature)).unwrap();
    let parsed = silico_core::naming::Name::parse(name, &|f| {
        model
            .document()
            .features
            .iter()
            .find(|x| &x.id == f)
            .map(|x| silico_core::naming::Maker::of(&x.operation))
    })
    .unwrap();
    let element = map.element(&parsed).unwrap();
    let description = model.description(&id(feature)).unwrap();
    match &description.edges[element.index as usize - 1].curve {
        EdgeCurve::Circle { axis, radius } => {
            assert_close("radius", *radius, 2.75, 1e-12);
            axis.origin
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_chamfer_on_copy_2s_rim_follows_a_change_of_spacing_and_breaks_loudly_below_3() {
    // patterns.md §7, on the real kernel: the rim of copy 2 named as section 3 names it,
    // chamfered; the spacing changed, the copy moves and the chamfer with it, removing the
    // same; the count lowered to 2, no copy 2: `reference_broken`, with candidates.
    let x = |extent: &str, count: u32| vec![direction("x", "extent", extent, count)];
    let model = built(&plate(&x("80", 3), &[rim("2")]));
    let pattern = summary(&model, "t0000001").volume;
    assert_close(
        "chamfered",
        pattern - summary(&model, "c0000001").volume,
        chamfered_away(),
        1e-6,
    );
    let centre = rim_centre(&model, "t0000001", &rim("2"));
    for (got, want) in centre.iter().zip([40.0, -20.0, 5.0]) {
        assert_close("rim at 80", *got, want, 1e-12);
    }
    let moved = built(&plate(&x("60", 3), &[rim("2")]));
    assert_close(
        "moved",
        summary(&moved, "t0000001").volume - summary(&moved, "c0000001").volume,
        chamfered_away(),
        1e-6,
    );
    assert_close(
        "rim at 60",
        rim_centre(&moved, "t0000001", &rim("2"))[0],
        20.0,
        1e-12,
    );
    let lowered = built(&plate(&x("80", 2), &[rim("2")]));
    let Some(FeatureOutcome::Broken { errors, .. }) = lowered.outcome(&id("c0000001")) else {
        panic!("{:?}", lowered.outcome(&id("c0000001")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert!(
        errors[0].candidates().contains(&rim("1")),
        "{:?}",
        errors[0].candidates()
    );
}

#[test]
fn a_grids_chamfered_copies_follow_its_directions_swapped_and_break_with_one_dropped() {
    // Section 10: a 3 × 3 grid, 40 along x by 20 along y, from the corner hole at
    // (−40, −20); a chamfer on (2, 1)'s rim at (40, 0) and another on (0, 1)'s at (−40, 0).
    // The directions swapped, (2, 1) is two steps along y and one along x: at (0, 20), its
    // chamfer removing the same. Direction 1 dropped, the one left y: `C1` is the hole at
    // (−40, 0), and both chamfers break, `C1`'s rim among `C0,1`'s candidates.
    let grid = [
        direction("x", "spacing", "40", 3),
        direction("y", "spacing", "20", 3),
    ];
    let model = built(&plate(&grid, &[rim("2,1"), rim("0,1")]));
    let away = summary(&model, "t0000001").volume - summary(&model, "c0000002").volume;
    assert_close("both", away, 2.0 * chamfered_away(), 1e-6);
    assert_eq!(
        rim_centre(&model, "t0000001", &rim("2,1"))[..2],
        [40.0, 0.0]
    );
    let swapped = [grid[1].clone(), grid[0].clone()];
    let model = built(&plate(&swapped, &[rim("2,1"), rim("0,1")]));
    let away = summary(&model, "t0000001").volume - summary(&model, "c0000002").volume;
    assert_close("swapped", away, 2.0 * chamfered_away(), 1e-6);
    let centre = rim_centre(&model, "t0000001", &rim("2,1"));
    assert!(centre[0].abs() < 1e-12, "{centre:?}");
    assert_close("y", centre[1], 20.0, 1e-12);
    let dropped = [grid[1].clone()];
    let model = built(&plate(&dropped, &[rim("2,1")]));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("c0000001")) else {
        panic!("{:?}", model.outcome(&id("c0000001")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    let model = built(&plate(&dropped, &[rim("0,1")]));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("c0000001")) else {
        panic!("{:?}", model.outcome(&id("c0000001")));
    };
    assert!(
        errors[0].candidates().contains(&rim("1")),
        "{:?}",
        errors[0].candidates()
    );
    let centre = rim_centre(&model, "t0000001", &rim("1"));
    assert_close("x", centre[0], -40.0, 1e-12);
    assert!(centre[1].abs() < 1e-12, "{centre:?}");
}

#[test]
fn a_pattern_rebuilt_on_another_engine_is_the_same_byte_for_byte() {
    // Section 9: placements from the integers, names from the tool's map and the step's
    // history: two engines, two pools, the same shape, measures, and names.
    let features = with(
        plate(
            &[
                direction("x", "extent", "80", 3),
                direction("y", "extent", "40", 2),
            ],
            &[],
        ),
        &[polar("t0000002", "t0000001", SOLID, "z", "90", 2)],
    );
    let (one, two) = (built(&features), built(&features));
    for feature in ["t0000001", "t0000002"] {
        let shape = |model: &Model| match model.outcome(&id(feature)) {
            Some(FeatureOutcome::Built { built, .. }) => built.shape.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(shape(&one), shape(&two), "{feature}");
        assert_eq!(summary(&one, feature), summary(&two, feature), "{feature}");
        let names = |model: &Model| {
            model
                .element_map(&id(feature))
                .unwrap()
                .iter()
                .map(|(element, names)| {
                    (
                        element,
                        names.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&one), names(&two), "{feature}");
    }
}

#[test]
fn a_copy_flush_inside_the_base_is_probed_and_its_verdict_recorded() {
    // Decision 6, *What the warning can miss*, probed (patterns.md records the verdict): a
    // boss 5 tall on a 10 mm plate's top, copied 5 down along −z, lies inside the plate, its
    // top flush with the plate's top. The pattern changes nothing for it, the plate's volume
    // and the boss's as before; whether its history says so is the probe's verdict.
    let boss = |count: u32| {
        vec![
            rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0),
            pad("p0000001", None, "s0000001", &up("10")),
            // On the plate's top, so the boss's tool is the boss, 10 to 15.
            polygon("s0000002", "xy", &[[5.0, 5.0], [10.0, 5.0], [10.0, 10.0], [5.0, 10.0]]).replace(
                r#""plane": "xy""#,
                r#""plane": {"face": "p0000001.pad.Face(G s0000001.sketch.Face1)", "x": "+x", "reversed": false}"#,
            ),
            pad("p0000002", Some("p0000001"), "s0000002", &up("5")),
            format!(
                r#"{{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "Down", "base": "p0000002", "copies": {}, "directions": [{{"along": "z", "reversed": true, "distribution": {{"type": "spacing", "length": 5}}, "count": {count}}}, {{"along": "x", "reversed": false, "distribution": {{"type": "spacing", "length": 20}}, "count": 2}}], "skip": []}}"#,
                of("p0000002")
            ),
        ]
    };
    // The copy flush inside alone: the step adds nothing, refused by S7's rule in its words
    // for a fuse step (section 4.2: "adds nothing to").
    let model = built(
        &boss(2)
            .into_iter()
            .map(|f| f.replace(r#""count": 2}], "skip""#, r#""count": 1}], "skip""#))
            .collect::<Vec<_>>(),
    );
    let error = failure(&model, "t0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    // As measured: the volume to the measure's last bit, and the counts the kernel gives.
    assert_eq!(
        error.message(),
        "linear_pattern t0000001 adds nothing to its base p0000002: the result's volume is the base's, 8124.999999999999 mm³ (11 faces, 24 edges, and 16 vertices, against the base's 11, 24, and 16)"
    );
    // (1): 5 down, flush inside; (0, 1): 20 along x, on the plate's top, adding a boss.
    let model = built(&boss(2));
    let verdicts = model.pattern_verdicts(&id("t0000001")).unwrap();
    let nothing: Vec<String> = verdicts[0]
        .changed_nothing
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_close(
        "volume",
        summary(&model, "t0000001").volume,
        8000.0 + 2.0 * 125.0,
        1e-9,
    );
    // The verdict, recorded in patterns.md: the flush copy (1) is caught, and so is (1, 1)
    // (5 down and 20 along, also flush inside), as no face of theirs is in the result.
    assert_eq!(nothing, ["1", "1,1"]);
    // Its warning, after the pattern's own fingerprint's (it stores none), as the codes are
    // ordered: a fuse step's words, "add nothing".
    let spared = model
        .document()
        .features
        .iter()
        .map(|f| f.id.clone())
        .filter(|f| f.as_str() != "t0000001")
        .collect();
    let warnings = silico_core::warnings::of(&model, &spared);
    let codes: Vec<silico_core::WarningCode> = warnings.listed().iter().map(|w| w.code()).collect();
    assert_eq!(
        codes,
        [
            silico_core::WarningCode::FingerprintMissing,
            silico_core::WarningCode::InstancesChangedNothing
        ]
    );
    let warning = &warnings.listed()[1];
    assert_eq!(
        warning.message(),
        "linear_pattern t0000001: the copies of instances 1 and 1,1 add nothing (no face of theirs is in the result)"
    );
    assert_eq!(warning.elements(), ["1", "1,1"]);
    assert_eq!(warning.parameter(), Some("skip"));
}

/// M4.1's block with a hole, its pin `height` tall, turned whole (`copies` of type `solid`)
/// by 90° about its hole's top rim, a circle (section 4.2's broken reference).
fn turned_about_its_rim(height: &str) -> Vec<String> {
    vec![
        r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#.to_owned(),
        format!(r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": {height}}}"#),
        r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#.to_owned(),
        polar(
            "t0000008",
            "m3tq8v0z",
            SOLID,
            "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)",
            "90",
            2,
        ),
    ]
}

#[test]
fn a_rim_as_an_axis_turns_about_its_circles_axis_and_breaks_loudly_once_it_moves_face() {
    // Section 4.2's example: the hole's top rim, a circle, the axis; the block turned 90°
    // about it crosses itself there: two 40 × 20 × 10 blocks, each less the hole, overlapping
    // in 20 × 20 × 10 less the hole, 2 · (8000 − hole) − (4000 − hole) = 12000 − hole, the
    // hole π · 2.5² · 10.
    let model = built(&turned_about_its_rim("12"));
    let hole = PI * 2.5 * 2.5 * 10.0;
    assert_close(
        "volume",
        summary(&model, "t0000008").volume,
        12_000.0 - hole,
        1e-9,
    );
    // The pin shortened to 6, the rim lies on the block's bottom: `reference_broken`, the
    // design's words byte for byte, its candidate the rim there.
    let model = built(&turned_about_its_rim("6"));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("t0000008")) else {
        panic!("{:?}", model.outcome(&id("t0000008")));
    };
    assert_eq!(errors.len(), 1);
    let got: serde_json::Value = serde_json::from_str(&written(&errors[0])).unwrap();
    let want: serde_json::Value = serde_json::from_str(&rebuild_examples()[0]).unwrap();
    assert_eq!(got, want);
}

#[test]
fn a_quarter_turn_about_a_cubes_edge_turns_right_handed_about_it_from_its_start_to_its_end() {
    // Decision 3: a straight edge as a polar axis runs from its start to its end, as
    // describe gives them, and the turn is right-handed about it. The 10 mm cube's side edge
    // at (10, 0), turned 90°: about +z, the copy lies over y = −10 to 0; about −z, over x =
    // 10 to 20. Each fuses with the cube on a face: 2,000.
    let edge = "p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000002)";
    let model = built(&with(
        cube(10.0),
        &[polar("t0000001", "p0000001", SOLID, edge, "90", 2)],
    ));
    let map = model.element_map(&id("p0000001")).unwrap();
    let parsed = silico_core::naming::Name::parse(edge, &|f| {
        model
            .document()
            .features
            .iter()
            .find(|x| &x.id == f)
            .map(|x| silico_core::naming::Maker::of(&x.operation))
    })
    .unwrap();
    let element = map.element(&parsed).unwrap();
    let described = &model.description(&id("p0000001")).unwrap().edges[element.index as usize - 1];
    assert!(matches!(described.curve, EdgeCurve::Line));
    let up = described.end[2] > described.start[2];
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, 2000.0, 1e-9);
    let [low, high] = s.bounds.unwrap();
    if up {
        assert_close("low y", low[1], -10.0, 1e-12);
        assert_close("high x", high[0], 10.0, 1e-12);
    } else {
        assert!(low[1].abs() < 1e-12, "{low:?}");
        assert_close("high x", high[0], 20.0, 1e-12);
    }
}

#[test]
fn two_originals_along_a_named_edge_each_resolve_it_in_their_own_step() {
    // Section 2.4: each of a pattern's steps resolves its references itself. Two originals,
    // a hole r 2.75 through the plate and a pocket r 1.5 4 deep, copied 20 along the plate's
    // front bottom edge (x, from its start at (−50, −30) to its end at (50, −30)): the second
    // step resolves the edge too, and both copies are cut, by hand.
    let features = vec![
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", &up("5")),
        circles("s0000003", "xy", &[([0.0, -20.0], 2.75)]),
        pocket("z0000001", "p0000001", "s0000003", THROUGH),
        circles("s0000004", "xy", &[([0.0, 0.0], 1.5)]),
        pocket(
            "a0000002",
            "z0000001",
            "s0000004",
            r#"{"type": "one_side", "length": 4, "reversed": true}"#,
        ),
        linear_as(
            "t0000001",
            "a0000002",
            r#"{"type": "features", "features": ["z0000001", "a0000002"]}"#,
            &[direction(
                "p0000001.pad.Edge(M s0000001.sketch.Edge.na000001)",
                "spacing",
                "20",
                2,
            )],
            "[]",
        ),
    ];
    let model = built(&features);
    let (hole, pocket_) = (PI * 2.75 * 2.75 * 5.0, PI * 1.5 * 1.5 * 4.0);
    let want = 30_000.0 - 2.0 * hole - 2.0 * pocket_;
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, want, 1e-9);
    // Both copies +20 along x, so the centre's x is −20 · (hole + pocket) / V by hand; one
    // copy the other way would put it at −20 · (hole − pocket) / V.
    let x = -20.0 * (hole + pocket_) / want;
    assert!(
        (s.center[0] - x).abs() < 1e-9,
        "{} against {x}",
        s.center[0]
    );
    let verdicts = model.pattern_verdicts(&id("t0000001")).unwrap();
    assert_eq!(verdicts.len(), 2);
}

#[test]
fn a_refused_patterns_copies_that_change_nothing_are_kept_but_never_warned_of() {
    // Section 4.3: the warning is a built pattern's. A hole near a plate's end mirrored about
    // yz lands off the plate: the step removes nothing, refused by S7's rule, its verdict
    // (instance 1 changed nothing) kept, and no warning beside the refusal.
    let model = built(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0),
        pad("p0000001", None, "s0000001", &up("5")),
        circles("s0000002", "xy", &[([35.0, 10.0], 2.75)]),
        pocket("q0000001", "p0000001", "s0000002", THROUGH),
        mirror("m0000001", "q0000001", &of("q0000001"), "yz"),
    ]);
    let error = failure(&model, "m0000001");
    assert!(
        error
            .message()
            .starts_with("mirror m0000001 removes nothing from its base q0000001"),
        "{error}"
    );
    let verdicts = model.pattern_verdicts(&id("m0000001")).unwrap();
    let nothing: Vec<String> = verdicts[0]
        .changed_nothing
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(nothing, ["1"]);
    let spared = model
        .document()
        .features
        .iter()
        .map(|f| f.id.clone())
        .collect();
    let warnings = silico_core::warnings::of(&model, &spared);
    assert!(warnings.listed().is_empty(), "{:?}", warnings.listed());
}

/// The described edge `name` of `feature`'s solid in `model`.
fn described_edge(model: &Model, feature: &str, name: &str) -> silico_protocol::EdgeDescription {
    let map = model.element_map(&id(feature)).unwrap();
    let parsed = silico_core::naming::Name::parse(name, &|f| {
        model
            .document()
            .features
            .iter()
            .find(|x| &x.id == f)
            .map(|x| silico_core::naming::Maker::of(&x.operation))
    })
    .unwrap();
    let element = map.element(&parsed).unwrap();
    model.description(&id(feature)).unwrap().edges[element.index as usize - 1].clone()
}

#[test]
fn a_sketch_made_edge_runs_as_its_profile_walks_and_a_pattern_along_it_goes_that_way() {
    // Decision 3's edges (Fable, 2026-11-24, docs/ports/patterns.md): an edge a pad makes
    // from a sketch's curve (a line, a circle, an arc) runs as the profile walks its loop
    // (M3.5's faces: an outer loop counter-clockwise, a hole clockwise), whichever way the
    // curve is drawn, where FreeCAD's
    // runs as drawn; the pattern takes describe's sense. A 40 x 20 block whose front line is
    // drawn from (40, 0) to (0, 0): its bottom edge runs from (0, 0, 0) to (40, 0, 0), and
    // the whole block copied 40 along it lies beside it, x = 0 to 80, 16,000, by hand.
    let drawn_back = rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0).replacen(
        r#""id": "na000001", "from": "pa000001", "to": "pa000002""#,
        r#""id": "na000001", "from": "pa000002", "to": "pa000001""#,
        1,
    );
    assert!(drawn_back.contains(r#""from": "pa000002", "to": "pa000001""#));
    let edge = "p0000001.pad.Edge(M s0000001.sketch.Edge.na000001)";
    let model = built(&[
        drawn_back,
        pad("p0000001", None, "s0000001", &up("10")),
        linear_as(
            "t0000001",
            "p0000001",
            SOLID,
            &[direction(edge, "spacing", "40", 2)],
            "[]",
        ),
    ]);
    let described = described_edge(&model, "p0000001", edge);
    assert!(matches!(described.curve, EdgeCurve::Line));
    for (got, want) in [
        (described.start, [0.0; 3]),
        (described.end, [40.0, 0.0, 0.0]),
    ] {
        assert!(
            (0..3).all(|k| (got[k] - want[k]).abs() < 1e-12),
            "{described:?}"
        );
    }
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, 16_000.0, 1e-9);
    let [low, high] = s.bounds.unwrap();
    assert!(low[0].abs() < 1e-12, "{low:?}");
    assert_close("high x", high[0], 80.0, 1e-12);

    // The same block with a hole r 2 at (10, 10) in its profile: the hole's circle is walked
    // clockwise, so its top edge's axis points down (-z), though drawn about +z; the block
    // turned a quarter turn right-handed about it, (x, y) to (y, 20 - x), lies over y = -20
    // to 20: 2 (8000 - 40 pi) - (4000 - 40 pi) = 12000 - 40 pi, by hand.
    let holed = r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Holed", "plane": "xy",
      "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 40, "y": 0, "construction": false}, {"id": "pa000003", "x": 40, "y": 20, "construction": false}, {"id": "pa000004", "x": 0, "y": 20, "construction": false}, {"id": "pa000005", "x": 10, "y": 10, "construction": false}],
      "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}],
      "circles": [{"id": "ca000001", "center": "pa000005", "radius": 2, "construction": false}], "arcs": [], "constraints": []}"#;
    let rim = "p0000002.pad.Edge(G s0000002.sketch.Edge.ca000001)";
    let model = built(&[
        holed.to_owned(),
        pad("p0000002", None, "s0000002", &up("10")),
        polar("t0000002", "p0000002", SOLID, rim, "90", 2),
    ]);
    let described = described_edge(&model, "p0000002", rim);
    let EdgeCurve::Circle { axis, .. } = described.curve else {
        panic!("{described:?}");
    };
    assert!(
        (0..3).all(|k| (axis.direction[k] - [0.0, 0.0, -1.0][k]).abs() < 1e-12),
        "{axis:?}"
    );
    let s = summary(&model, "t0000002");
    assert_close("volume", s.volume, 12_000.0 - 40.0 * PI, 1e-9);
    let [low, high] = s.bounds.unwrap();
    assert_close("low y", low[1], -20.0, 1e-12);
    assert_close("high y", high[1], 20.0, 1e-12);

    // An arc walked clockwise: a slot hole's right end, drawn counter-clockwise about
    // (12, 10) from (12, 8) to (12, 12), its edge's axis -z; the block turned a quarter turn
    // about it, (x, y) to (y + 2, 22 - x), lies over y = -18 to 22: 11960 - 30 pi, by hand
    // (the two slots' union 28 + 5 pi; tests/oracle/patterns.silico's slot_arc).
    let slotted = r#"{"id": "s0000003", "type": "sketch", "v": 1, "name": "Slotted", "plane": "xy",
      "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 40, "y": 0, "construction": false}, {"id": "pa000003", "x": 40, "y": 20, "construction": false}, {"id": "pa000004", "x": 0, "y": 20, "construction": false}, {"id": "pa000005", "x": 8, "y": 8, "construction": false}, {"id": "pa000006", "x": 12, "y": 8, "construction": false}, {"id": "pa000007", "x": 12, "y": 12, "construction": false}, {"id": "pa000008", "x": 8, "y": 12, "construction": false}, {"id": "pa000009", "x": 12, "y": 10, "construction": false}, {"id": "pa000010", "x": 8, "y": 10, "construction": false}],
      "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000007", "to": "pa000008", "construction": false}],
      "circles": [], "arcs": [{"id": "ra000001", "center": "pa000009", "start": "pa000006", "end": "pa000007", "construction": false}, {"id": "ra000002", "center": "pa000010", "start": "pa000008", "end": "pa000005", "construction": false}], "constraints": []}"#;
    let arc = "p0000003.pad.Edge(G s0000003.sketch.Edge.ra000001)";
    let model = built(&[
        slotted.to_owned(),
        pad("p0000003", None, "s0000003", &up("10")),
        polar("t0000003", "p0000003", SOLID, arc, "90", 2),
    ]);
    let described = described_edge(&model, "p0000003", arc);
    let EdgeCurve::Circle { axis, .. } = described.curve else {
        panic!("{described:?}");
    };
    assert!(
        (0..3).all(|k| (axis.direction[k] - [0.0, 0.0, -1.0][k]).abs() < 1e-12),
        "{axis:?}"
    );
    let s = summary(&model, "t0000003");
    assert_close("volume", s.volume, 11_960.0 - 30.0 * PI, 1e-9);
    let [low, high] = s.bounds.unwrap();
    assert_close("low y", low[1], -18.0, 1e-12);
    assert_close("high y", high[1], 22.0, 1e-12);
}
