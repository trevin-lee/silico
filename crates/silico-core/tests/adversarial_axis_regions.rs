//! The round-2 tests review's proofs (each an evil `check_axis` or `checked()` that passed
//! every test before): the axis is checked region by region (the crossing for each region
//! alone, naming only its curves; the angle against the farthest point of all), in both
//! orders of the regions; and a sweep feature whose solid, or whose base's, can't be
//! measured can't be checked, so it fails with the measure's error, naming itself.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "common/sweeps.rs"]
mod sweeps;

use std::sync::Arc;
use std::time::Duration;

use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::Op;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use sweeps::*;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<FakeKernel>, features: &[&str]) -> Arc<Model> {
    let document = checked_document(features);
    match engine.start(document).wait_timeout(Duration::from_secs(60)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

/// A sketch on XY of two rectangles, `a` (points pa, lines na) then `b` (pb, nb), each
/// `[x0, y0, x1, y1]`.
fn two(a: [f64; 4], b: [f64; 4]) -> String {
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for (tag, [x0, y0, x1, y1]) in [("a", a), ("b", b)] {
        let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        for (i, [x, y]) in corners.iter().enumerate() {
            points.push(format!(
                r#"{{"id": "p{tag}00000{}", "x": {x}, "y": {y}, "construction": false}}"#,
                i + 1
            ));
        }
        for i in 0..4 {
            lines.push(format!(
                r#"{{"id": "n{tag}00000{}", "from": "p{tag}00000{}", "to": "p{tag}00000{}", "construction": false}}"#,
                i + 1,
                i + 1,
                (i + 1) % 4 + 1
            ));
        }
    }
    format!(
        r#"{{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}

fn base() -> String {
    box_at("bbbbbbbb", [-1.0, -120.0, -120.0], 20.0, 240.0, 240.0)
}

fn turn(angle_text: &str) -> String {
    turned(
        "rrrrrrrr",
        "groove",
        Some("bbbbbbbb"),
        "ssssssss",
        "x_axis",
        &angle(angle_text),
    )
}

/// A region crossing the axis is refused whichever region it is, naming only its own
/// curves: one rectangle above the x axis (y 1..2), one across it (y −1..1).
#[test]
fn a_crossing_region_is_refused_whatever_its_place_naming_only_its_curves() {
    let clean = [0.0, 1.0, 4.0, 2.0];
    let across = [10.0, -1.0, 14.0, 1.0];
    for (sketch, tag) in [(two(clean, across), "nb"), (two(across, clean), "na")] {
        let model = published(&engine(), &[&base(), &sketch, &turn("90")]);
        let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("rrrrrrrr")) else {
            panic!("{tag}: {:?}", model.outcome(&id("rrrrrrrr")));
        };
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert!(error.message().contains("crosses its profile"), "{error:?}");
        // Every side of the crossing rectangle reaches off the axis: all four.
        let want: Vec<String> = (1..=4).map(|i| format!("{tag}00000{i}")).collect();
        assert_eq!(error.elements(), want, "{tag}");
        assert_eq!(
            error.message(),
            "groove rrrrrrrr turns sketch ssssssss about a line that crosses its profile: its axis, the sketch's x_axis, has the profile's curves on both sides, 1 mm on one and 1 mm on the other"
        );
        assert_eq!(error.feature(), Some("ssssssss"));
        assert_eq!(error.elements_total(), 4);
    }
}

/// The angle's check takes the farthest point over every region: a region 1 mm from the
/// axis and one 100 mm from it, turned 1e-6 degrees (1.745e-8 rad): the far one moves
/// 1.745e-6 mm, more than 1e-7, so the revolve isn't refused for its angle, whichever
/// region comes first.
#[test]
fn the_angle_is_checked_against_the_farthest_region_whatever_its_place() {
    let near = [0.0, 0.0, 4.0, 1.0];
    let far = [10.0, 0.0, 14.0, 100.0];
    for (sketch, order) in [
        (two(near, far), "near first"),
        (two(far, near), "far first"),
    ] {
        let model = published(&engine(), &[&base(), &sketch, &turn("0.000001")]);
        // Past the axis's checks to the kernel: built, or refused by the result's volume
        // check (the fake's groove at so small an angle removes too little), never by the
        // angle's.
        match model.outcome(&id("rrrrrrrr")) {
            Some(FeatureOutcome::Built { .. }) => {}
            Some(FeatureOutcome::Failed(error)) => {
                assert_ne!(
                    error.parameter(),
                    Some("extent.angle"),
                    "{order}: {error:?}"
                );
                assert_eq!(error.code(), ErrorCode::KernelFailed, "{order}: {error:?}");
            }
            other => panic!("{order}: {other:?}"),
        }
    }
}

/// A pad onto a box whose solids can't be measured: the pad can't be checked, so it fails
/// with the measure's error, naming the pad (checked()'s unmeasured branch).
#[test]
fn a_sweep_feature_whose_solid_cant_be_measured_fails_with_why() {
    let engine = engine();
    engine.kernel().refuse(|op| {
        matches!(op, Op::Summarize { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "Summarize failed"))
    });
    let model = published(
        &engine,
        &[
            &box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0),
            &rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
            &pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN),
        ],
    );
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("pppppppp")) else {
        panic!("{:?}", model.outcome(&id("pppppppp")));
    };
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(error.feature(), Some("pppppppp"));
    assert!(error.message().contains("Summarize failed"), "{error:?}");
}

/// The same, but only the base's measure fails: the pad's own solid is measured, its base's
/// isn't, so the base's error is the one said (the base-key fallback), naming the pad.
#[test]
fn a_sweep_feature_whose_bases_solid_cant_be_measured_fails_with_why() {
    let engine = engine();
    engine.kernel().refuse(|op| match op {
        Op::Summarize { shape }
            if FakeKernel::describe(shape).is_some_and(|d| d.starts_with("translate(box")) =>
        {
            Some(Error::new(
                ErrorCode::KernelFailed,
                "the base's Summarize failed",
            ))
        }
        _ => None,
    });
    let model = published(
        &engine,
        &[
            &box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0),
            &rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
            &pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN),
        ],
    );
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("pppppppp")) else {
        panic!("{:?}", model.outcome(&id("pppppppp")));
    };
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(error.feature(), Some("pppppppp"));
    assert!(
        error.message().contains("the base's Summarize failed"),
        "{error:?}"
    );
}
