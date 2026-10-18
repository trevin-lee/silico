//! M3.6: what a sketch feature's rebuild refuses (docs/design/m3.6-sketch-features.md, S5 and
//! S7), as pure functions: a pad or revolve of several regions (decision 3), a result of more
//! than one solid, and a result whose volume shows the feature changed nothing. Each
//! boundary from the design's own numbers: on a 1000 mm³ base, 1e-9 of it is 1e-6 mm³.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::ErrorCode;
use silico_core::document::SweepFeature;
use silico_core::document::{ElementId, FeatureId, Plane};
use silico_core::profile::{CurveKind, Edge, Loop, Profile, Region};
use silico_core::sweep::{Measured, check_regions, check_result};
use silico_protocol::{Curve, MEASURE_PRECISION};

fn fid(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

const FEATURE: &str = "p0000001";
const BASE: &str = "b0000001";

/// A 10 mm cube, as the kernel measures it.
fn cube() -> Measured {
    Measured {
        volume: 1000.0,
        solids: 1,
        faces: 6,
        edges: 12,
        vertices: 8,
    }
}

/// A result of `volume`, with the counts of a pad on the cube's top (or a pocket in it).
fn result(volume: f64) -> Measured {
    Measured {
        volume,
        solids: 1,
        faces: 10,
        edges: 20,
        vertices: 12,
    }
}

fn check(kind: SweepFeature, before: Measured, after: Measured) -> Result<(), silico_core::Error> {
    check_result(&fid(FEATURE), kind, &fid(BASE), &before, &after)
}

#[test]
fn the_precision_is_the_precise_measures_one_in_a_billion() {
    assert_eq!(MEASURE_PRECISION, 1e-9);
}

#[test]
fn a_pad_or_revolve_adding_more_than_a_billionth_of_its_base_passes_and_no_more_is_refused() {
    for kind in [SweepFeature::Pad, SweepFeature::Revolve] {
        // On 1000 mm³, a billionth is 1e-6 mm³: a hair over passes, a hair under doesn't.
        assert_eq!(check(kind, cube(), result(1000.0 + 1.01e-6)), Ok(()));
        let refused = check(kind, cube(), result(1000.0 + 0.99e-6)).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::KernelFailed);
        assert_eq!(refused.feature(), Some(FEATURE));
        // Exactly a billionth (1e9 × 1e-9 is exactly 1): no more than it, refused; twice it
        // passes.
        let big = Measured {
            volume: 1e9,
            ..cube()
        };
        assert!(check(kind, big, result(1e9 + 1.0)).is_err());
        assert_eq!(check(kind, big, result(1e9 + 2.0)), Ok(()));
        // The base alone, and less than it (a fuse never removes), refused.
        assert!(check(kind, cube(), result(1000.0)).is_err());
        assert!(check(kind, cube(), result(995.0)).is_err());
        // A real pad: 2 × 2 × 2 on top of the cube.
        assert_eq!(check(kind, cube(), result(1008.0)), Ok(()));
    }
}

#[test]
fn a_pocket_or_groove_removing_more_than_a_billionth_of_its_base_passes_and_no_more_is_refused() {
    for kind in [SweepFeature::Pocket, SweepFeature::Groove] {
        assert_eq!(check(kind, cube(), result(1000.0 - 1.01e-6)), Ok(()));
        let refused = check(kind, cube(), result(1000.0 - 0.99e-6)).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::KernelFailed);
        let big = Measured {
            volume: 1e9,
            ..cube()
        };
        assert!(check(kind, big, result(1e9 - 1.0)).is_err());
        assert_eq!(check(kind, big, result(1e9 - 2.0)), Ok(()));
        // The base alone, and more than it (a cut never adds), refused.
        assert!(check(kind, cube(), result(1000.0)).is_err());
        assert!(check(kind, cube(), result(1008.0)).is_err());
        // A real pocket: 2 × 2 × 2 out of the cube.
        assert_eq!(check(kind, cube(), result(992.0)), Ok(()));
    }
}

#[test]
fn the_answer_is_the_volumes_whatever_the_counts() {
    // A wrong-way pad from the base's top face: the top split along the outline, 7 faces at
    // the base's volume (measured on the oracle's FreeCAD): refused though the counts moved.
    let scarred = Measured {
        faces: 7,
        edges: 15,
        vertices: 10,
        ..cube()
    };
    let refused = check(SweepFeature::Pad, cube(), scarred).unwrap_err();
    assert_eq!(
        refused.message(),
        "pad p0000001 adds nothing to its base b0000001: the result's volume is the base's, 1000 mm³ (7 faces, 15 edges, and 10 vertices, against the base's 6, 12, and 8)"
    );
    assert_eq!(
        refused.suggestion(),
        Some(
            "if the pad points into its base, reverse it; if it lies inside it, move its sketch; if neither, OCCT lost it: a thin wall of its profile flush with a face of the base can do this (docs/ports/sketch-features.md), so thicken it"
        )
    );
    assert!(refused.elements().is_empty());
    assert_eq!(refused.parameter(), None);
    // A pocket and a pad of the same counts as their base, changing its volume: passing (a
    // 2 mm pocket in a 10 mm cube, then a symmetric pad of 4 of the same square: 992, 1008).
    let pocketed = Measured {
        volume: 992.0,
        ..cube()
    };
    assert_eq!(check(SweepFeature::Pocket, cube(), pocketed), Ok(()));
    assert_eq!(
        check(
            SweepFeature::Pad,
            pocketed,
            Measured {
                volume: 1008.0,
                ..cube()
            }
        ),
        Ok(())
    );
    let removes = check(SweepFeature::Groove, cube(), cube()).unwrap_err();
    assert_eq!(
        removes.message(),
        "groove p0000001 removes nothing from its base b0000001: the result's volume is the base's, 1000 mm³ (6 faces, 12 edges, and 8 vertices, against the base's 6, 12, and 8)"
    );
    assert!(
        removes
            .suggestion()
            .unwrap()
            .starts_with("if the groove points away from its base, reverse it")
    );
}

#[test]
fn the_precision_scales_with_the_base() {
    // On a 1e6 mm³ base, a billionth is 1e-3: 0.5e-3 added is nothing, 2e-3 is something.
    let base = Measured {
        volume: 1e6,
        ..cube()
    };
    assert!(check(SweepFeature::Pad, base, result(1e6 + 0.5e-3)).is_err());
    assert_eq!(check(SweepFeature::Pad, base, result(1e6 + 2e-3)), Ok(()));
    // Ten times the precision would refuse 5e-6 on the cube; a billionth passes it.
    assert_eq!(
        check(SweepFeature::Pad, cube(), result(1000.0 + 5e-6)),
        Ok(())
    );
    assert_eq!(
        check(SweepFeature::Pocket, cube(), result(1000.0 - 5e-6)),
        Ok(())
    );
}

#[test]
fn one_solid_passes_and_two_or_none_are_refused_naming_the_count() {
    let solids = |n: u32| Measured {
        solids: n,
        ..result(1100.0)
    };
    assert_eq!(check(SweepFeature::Pad, cube(), solids(1)), Ok(()));
    let missed = check(SweepFeature::Pad, cube(), solids(2)).unwrap_err();
    assert_eq!(missed.code(), ErrorCode::KernelFailed);
    assert_eq!(missed.feature(), Some(FEATURE));
    assert_eq!(
        missed.message(),
        "pad p0000001 doesn't reach its base b0000001: the result is 2 solids"
    );
    assert_eq!(
        missed.suggestion(),
        Some("move the sketch or lengthen the pad, or pad it with no base")
    );
    let revolve = check(SweepFeature::Revolve, cube(), solids(3)).unwrap_err();
    assert_eq!(
        revolve.message(),
        "revolve p0000001 doesn't reach its base b0000001: the result is 3 solids"
    );
    let cut = Measured {
        solids: 2,
        ..result(900.0)
    };
    let pocket = check(SweepFeature::Pocket, cube(), cut).unwrap_err();
    assert_eq!(
        pocket.message(),
        "pocket p0000001 cuts its base b0000001 in two: the result is 2 solids"
    );
    assert_eq!(
        pocket.suggestion(),
        Some("move the sketch or shorten the pocket")
    );
    assert_eq!(
        check(SweepFeature::Groove, cube(), Measured { solids: 4, ..cut })
            .unwrap_err()
            .message(),
        "groove p0000001 cuts its base b0000001 in 4: the result is 4 solids"
    );
    // The solid count is checked first: two solids at the base's volume say they're two.
    let both = Measured {
        solids: 2,
        ..cube()
    };
    assert!(
        check(SweepFeature::Pad, cube(), both)
            .unwrap_err()
            .message()
            .ends_with("the result is 2 solids")
    );
    // None: the kernel refuses an empty boolean first; refused here too.
    for kind in SweepFeature::ALL {
        let none = check(
            kind,
            cube(),
            Measured {
                solids: 0,
                volume: 0.0,
                ..cube()
            },
        )
        .unwrap_err();
        assert_eq!(none.code(), ErrorCode::KernelFailed);
        assert!(
            none.message().ends_with("the result is no solid"),
            "{none:?}"
        );
    }
}

#[test]
fn each_feature_knows_whether_it_adds() {
    let adds: Vec<bool> = SweepFeature::ALL.iter().map(|k| k.adds()).collect();
    assert_eq!(adds, [true, false, true, false]);
    let words: Vec<&str> = SweepFeature::ALL.iter().map(|k| k.word()).collect();
    assert_eq!(words, ["pad", "pocket", "revolve", "groove"]);
}

// --- Decision 3 --------------------------------------------------------------------------

fn eid(n: usize) -> ElementId {
    ElementId::new(&format!("c{n:07}")).unwrap()
}

/// A region whose outer loop is a square of side `w` from `(x, 0)`, its curves `first`,
/// `first + 1`, ... (sketch IDs `c0000001` on).
fn square(first: usize, x: f64, w: f64) -> Region {
    let p = [[x, 0.0], [x + w, 0.0], [x + w, w], [x, w]];
    let curves = (0..4)
        .map(|i| Edge {
            curve: eid(first + i),
            kind: CurveKind::Line,
            center: None,
            ends: Some([eid(900 + first + i), eid(900 + first + (i + 1) % 4)]),
            reversed: false,
            sent: Curve::Line {
                start: p[i],
                end: p[(i + 1) % 4],
            },
        })
        .collect();
    Region {
        outer: Loop {
            curves,
            area: w * w,
            perimeter: 4.0 * w,
        },
        holes: Vec::new(),
        depth: 0,
    }
}

/// A region of one circle, curve `n`.
fn round(n: usize, x: f64) -> Region {
    Region {
        outer: Loop {
            curves: vec![Edge {
                curve: eid(n),
                kind: CurveKind::Circle,
                center: Some(eid(800 + n)),
                ends: None,
                reversed: false,
                sent: Curve::Circle {
                    center: [x, 0.0],
                    radius: 15.0,
                },
            }],
            area: 225.0 * std::f64::consts::PI,
            perimeter: 30.0 * std::f64::consts::PI,
        },
        holes: Vec::new(),
        depth: 0,
    }
}

fn regions(regions: Vec<Region>) -> Profile {
    Profile {
        plane: Plane::Xy,
        regions,
    }
}

fn check_profile(kind: SweepFeature, profile: &Profile) -> Result<(), silico_core::Error> {
    check_regions(&fid(FEATURE), kind, &fid("s0000001"), profile)
}

#[test]
fn a_pad_or_revolve_of_one_region_passes_with_holes_or_not() {
    let mut holed = square(1, 0.0, 10.0);
    holed.holes.push(round(5, 5.0).outer);
    for profile in [regions(vec![square(1, 0.0, 10.0)]), regions(vec![holed])] {
        for kind in SweepFeature::ALL {
            assert_eq!(check_profile(kind, &profile), Ok(()), "{kind:?}");
        }
    }
}

#[test]
fn a_pad_or_revolve_of_two_regions_is_refused_naming_every_outer_curve_and_their_total() {
    let two = regions(vec![square(1, 0.0, 10.0), square(5, 20.0, 10.0)]);
    let refused = check_profile(SweepFeature::Pad, &two).unwrap_err();
    assert_eq!(refused.code(), ErrorCode::InvalidInput);
    // The sketch's, since that's where the regions are.
    assert_eq!(refused.feature(), Some("s0000001"));
    assert_eq!(
        refused.elements(),
        [
            "c0000001", "c0000002", "c0000003", "c0000004", "c0000005", "c0000006", "c0000007",
            "c0000008"
        ]
    );
    assert_eq!(refused.elements_total(), 8);
    assert_eq!(
        refused.message(),
        "sketch s0000001 makes 2 separate regions (inside loops c0000001, c0000002, c0000003, c0000004 and c0000005, c0000006, c0000007, c0000008), and pad p0000001 makes one solid of one region"
    );
    assert_eq!(
        refused.suggestion(),
        Some(
            "pad each region from its own sketch, or join them into one region; a pocket takes several"
        )
    );
    let revolve = check_profile(SweepFeature::Revolve, &two).unwrap_err();
    assert_eq!(revolve.elements_total(), 8);
    assert!(
        revolve
            .message()
            .ends_with("and revolve p0000001 makes one solid of one region")
    );
    assert_eq!(
        revolve.suggestion(),
        Some(
            "revolve each region from its own sketch, or join them into one region; a groove takes several"
        )
    );
    // A pocket and a groove take both.
    assert_eq!(check_profile(SweepFeature::Pocket, &two), Ok(()));
    assert_eq!(check_profile(SweepFeature::Groove, &two), Ok(()));
}

#[test]
fn freecads_four_circles_are_four_regions_named() {
    // testPD_TNPSketchPadMultipleSolids (commented out there): four separate circles r 15.
    let four = regions((0..4).map(|i| round(i + 1, 40.0 * i as f64)).collect());
    let refused = check_profile(SweepFeature::Pad, &four).unwrap_err();
    assert_eq!(
        refused.elements(),
        ["c0000001", "c0000002", "c0000003", "c0000004"]
    );
    assert_eq!(refused.elements_total(), 4);
    assert!(
        refused.message().starts_with(
            "sketch s0000001 makes 4 separate regions (inside loops c0000001; c0000002; c0000003; and c0000004), and pad"
        ),
        "{}",
        refused.message()
    );
}

#[test]
fn a_profile_of_many_regions_names_the_first_hundred_curves_and_counts_them_all() {
    // 30 squares: 120 outer curves.
    let many = regions(
        (0..30)
            .map(|i| square(4 * i + 1, 20.0 * i as f64, 10.0))
            .collect(),
    );
    let refused = check_profile(SweepFeature::Pad, &many).unwrap_err();
    assert_eq!(refused.elements().len(), 100);
    assert_eq!(refused.elements()[99], "c0000100");
    assert_eq!(refused.elements_total(), 120);
    assert!(
        refused
            .message()
            .contains("c0000097, c0000098, c0000099, c0000100, and 20 more curves), and pad"),
        "{}",
        refused.message()
    );
    assert!(!refused.message().contains("c0000101"));
}
