//! M3.6: what a sketch feature's rebuild refuses (docs/design/m3.6-sketch-features.md, S5 and
//! S7), as pure functions: a pad or revolve of several regions (decision 3), a result of more
//! than one solid, a result whose volume shows the feature changed nothing, and a sweep of a
//! sketch that makes no profile (S3). Each
//! boundary from the design's own numbers: on a 1000 mm³ base, 1e-9 of it is 1e-6 mm³.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::draw;

use silico_core::ErrorCode;
use silico_core::document::SweepFeature;
use silico_core::document::{ElementId, FeatureId, OriginPlane};
use silico_core::profile::{CurveKind, Edge, Loop, Profile, Region};
use silico_core::sweep::{Measured, check_regions, check_result, unprofiled};
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
fn a_result_lost_or_parted_gives_each_features_own_advice() {
    // What to try, in each feature's words, when the result is no solid or several: a sweep
    // that adds was lost by OCCT with its base, one that cuts took all of its base; an M2
    // boolean's tool is to move or shrink for either, and to move when it parts its base.
    use silico_core::sweep::check_boolean_change;
    use silico_protocol::BooleanOp;
    let lost = Measured {
        solids: 0,
        volume: 0.0,
        ..cube()
    };
    for (kind, advice) in [
        (
            SweepFeature::Pad,
            "OCCT lost both shapes: move the sketch slightly",
        ),
        (
            SweepFeature::Revolve,
            "OCCT lost both shapes: move the sketch slightly",
        ),
        (
            SweepFeature::Pocket,
            "move the sketch or shorten the cut, so it leaves some of its base",
        ),
        (
            SweepFeature::Groove,
            "move the sketch or shorten the cut, so it leaves some of its base",
        ),
    ] {
        let refused = check(kind, cube(), lost).unwrap_err();
        assert_eq!(refused.suggestion(), Some(advice), "{kind:?}");
    }
    let boolean = |op, after: Measured| {
        check_boolean_change(&fid(FEATURE), op, &fid(BASE), &cube(), &after).unwrap_err()
    };
    let parted = Measured {
        solids: 2,
        ..result(900.0)
    };
    for op in [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common] {
        let none = boolean(op, lost);
        assert_eq!(none.code(), ErrorCode::KernelFailed);
        assert_eq!(
            none.message(),
            format!(
                "{} p0000001 leaves nothing of its base b0000001: the result is no solid",
                op.word()
            )
        );
        assert_eq!(
            none.suggestion(),
            Some("move or shrink the tool, so the result keeps some of its base"),
            "{op:?}"
        );
        let two = boolean(op, parted);
        assert_eq!(two.code(), ErrorCode::KernelFailed);
        assert_eq!(
            two.suggestion(),
            Some("move the tool so the result is one piece"),
            "{op:?}"
        );
    }
    assert_eq!(
        boolean(BooleanOp::Fuse, parted).message(),
        "fuse p0000001 doesn't reach its base b0000001: the result is 2 solids"
    );
    assert_eq!(
        boolean(BooleanOp::Cut, parted).message(),
        "cut p0000001 cuts its base b0000001 in two: the result is 2 solids"
    );
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
        plane: OriginPlane::Xy.into(),
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
    // The pad's own error; its elements the sketch's, since that's where the regions are.
    assert_eq!(refused.feature(), Some("p0000001"));
    assert_eq!(refused.parameter(), Some("sketch"));
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

#[test]
fn a_change_under_the_precision_is_said_as_the_change_and_the_limit() {
    // Found by break-it: a 4 by 2 hole through a 200 m cube removes 1.6e6 mm³, no more than
    // 1e-9 of the cube's 8e15 (8e6): refused by the rule (S7, S11), but the message said
    // the result's volume was the base's. Now it says the change and the limit, with advice
    // that fits a small feature on a large part. Worked by hand: 8e15 × 1e-9 = 8e6.
    let base = Measured {
        volume: 8e15,
        ..cube()
    };
    let refused = check(SweepFeature::Pocket, base, result(8e15 - 1.6e6)).unwrap_err();
    assert_eq!(
        refused.message(),
        "pocket p0000001 removes nothing from its base b0000001 that its volume can tell: the 1600000 mm³ it removes is no more than the 8000000 mm³ (1e-9 of the base's volume) the volume is measured to (10 faces, 20 edges, and 12 vertices, against the base's 6, 12, and 8)"
    );
    assert!(
        refused
            .suggestion()
            .unwrap()
            .starts_with("make the pocket larger, or make it on a smaller part"),
        "{:?}",
        refused.suggestion()
    );
    // No change at all keeps the words it had: the volume is the base's.
    let same = check(SweepFeature::Pocket, base, result(8e15)).unwrap_err();
    assert!(same.message().contains("the result's volume is the base's"));
}

#[test]
fn an_m2_booleans_change_more_than_a_billionth_of_its_base_passes_and_no_more_is_refused() {
    // S7's limit for M2 booleans, as S9's test holds it for sweeps: on 1000 mm³, a billionth
    // is 1e-6 mm³. A cut and a common take from their base, a fuse adds to it: a hair over
    // the limit the right way passes, a hair under is refused, and the wrong way is refused
    // however far. `boolean_change_holds` agrees with `check_boolean_change` every time.
    use silico_core::sweep::{boolean_change_holds, check_boolean_change};
    use silico_protocol::BooleanOp;
    for (op, sign) in [
        (BooleanOp::Cut, -1.0),
        (BooleanOp::Common, -1.0),
        (BooleanOp::Fuse, 1.0),
    ] {
        for (change, holds) in [
            (1.01e-6, true),
            (0.99e-6, false),
            (-1.01e-6, false),
            (-1.0, false),
            (0.0, false),
        ] {
            let after = result(1000.0 + sign * change);
            let checked = check_boolean_change(&fid(FEATURE), op, &fid(BASE), &cube(), &after);
            assert_eq!(checked.is_ok(), holds, "{op:?} by {change}: {checked:?}");
            assert_eq!(
                boolean_change_holds(op, &cube(), &after),
                holds,
                "{op:?} by {change}"
            );
            if let Err(e) = checked {
                assert_eq!(e.code(), ErrorCode::KernelFailed);
                assert_eq!(e.feature(), Some(FEATURE));
            }
        }
    }
}

#[test]
fn a_change_the_wrong_way_says_both_volumes() {
    // A fuse whose result measures less than its base, and a cut whose result measures
    // more, as a broken result or a forged reply would: refused by S7's rule, saying both
    // volumes and which way, not that they're the same. A sketch feature the same.
    use silico_core::sweep::check_boolean_change;
    use silico_protocol::BooleanOp;
    let fused = check_boolean_change(
        &fid(FEATURE),
        BooleanOp::Fuse,
        &fid(BASE),
        &cube(),
        &result(999.5),
    )
    .unwrap_err();
    assert_eq!(
        fused.message(),
        "fuse p0000001 adds nothing to its base b0000001: the result's volume, 999.5 mm³, is less than the base's, 1000 mm³ (10 faces, 20 edges, and 12 vertices, against the base's 6, 12, and 8)"
    );
    let cut = check_boolean_change(
        &fid(FEATURE),
        BooleanOp::Cut,
        &fid(BASE),
        &cube(),
        &result(1001.0),
    )
    .unwrap_err();
    assert_eq!(
        cut.message(),
        "cut p0000001 removes nothing from its base b0000001: the result's volume, 1001 mm³, is more than the base's, 1000 mm³ (10 faces, 20 edges, and 12 vertices, against the base's 6, 12, and 8)"
    );
    let pad = check(SweepFeature::Pad, cube(), result(999.5)).unwrap_err();
    assert!(
        pad.message()
            .contains("the result's volume, 999.5 mm³, is less than the base's, 1000 mm³"),
        "{}",
        pad.message()
    );
    // The same volume keeps its words.
    let same = check_boolean_change(
        &fid(FEATURE),
        BooleanOp::Common,
        &fid(BASE),
        &cube(),
        &result(1000.0),
    )
    .unwrap_err();
    assert!(
        same.message()
            .contains("common p0000001 removes nothing from its base b0000001: the result's volume is the base's, 1000 mm³"),
        "{}",
        same.message()
    );
}

#[test]
fn an_m2_boolean_of_more_than_one_solid_is_refused_naming_itself_and_the_cause() {
    // D2's letter for M2's booleans (Fable's decision; design S5), on histories written by
    // hand: a result of one solid passes, of two or more is refused, `kernel_failed`, in
    // the words Fable gave each operation.
    use silico_core::kernel::{ElementCounts, History};
    use silico_core::sweep::check_boolean_solids;
    use silico_protocol::BooleanOp;
    let history = |solids: u32| History {
        inputs: Vec::new(),
        result: ElementCounts {
            faces: 12,
            edges: 24,
            vertices: 16,
            solids,
        },
        ends: Default::default(),
        topology: Default::default(),
    };
    let check = |op, solids: u32| {
        check_boolean_solids(
            &fid("f0000001"),
            op,
            &fid("b0000001"),
            &fid("b0000002"),
            history(solids).result.solids,
        )
    };
    for op in [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common] {
        assert_eq!(check(op, 1), Ok(()), "{op:?}");
        // None is the kernel's empty boolean's to refuse, before this.
        assert_eq!(check(op, 0), Ok(()), "{op:?}");
        let refused = check(op, 2).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::KernelFailed);
        assert_eq!(refused.feature(), Some("f0000001"));
    }
    let said = |op, solids| {
        let e = check(op, solids).unwrap_err();
        (e.message().to_owned(), e.suggestion().unwrap().to_owned())
    };
    assert_eq!(
        said(BooleanOp::Fuse, 2),
        (
            "fuse f0000001 makes 2 solids: its base b0000001 and tool b0000002 don't touch, or meet only along an edge or at a point".to_owned(),
            "move them to overlap or share a face; a solid nothing can address can't be made until bodies exist".to_owned()
        )
    );
    assert_eq!(
        said(BooleanOp::Cut, 2),
        (
            "cut f0000001 cuts its base b0000001 in two: the result is 2 solids".to_owned(),
            "shorten or move the tool so the base stays one piece".to_owned()
        )
    );
    assert_eq!(
        said(BooleanOp::Common, 2).0,
        "common f0000001 makes 2 solids of base b0000001 and tool b0000002: they overlap in two places"
    );
    assert_eq!(
        said(BooleanOp::Cut, 3).0,
        "cut f0000001 cuts its base b0000001 in three: the result is 3 solids"
    );
}

#[test]
fn a_sweep_of_a_sketch_that_makes_no_profile_is_one_error_naming_it_however_many_refusals() {
    // One open line, (0, 0) to (4, 0): its two ends, p0000001 and p0000002, are open, two
    // refusals of three elements (the line, n0000003, and its ends); a groove of it is one
    // error naming the groove, the sketch's elements its own, the two refusals and their
    // advice its cause.
    let mut d = draw::Draw::new();
    let a = d.point(0.0, 0.0);
    let b = d.point(4.0, 0.0);
    d.line(&a, &b);
    let refused = d.profile().unwrap_err();
    assert_eq!(refused.total(), 2);
    let error = unprofiled(
        &fid("g0000001"),
        SweepFeature::Groove,
        &draw::feature(),
        &refused,
    );
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some("g0000001"));
    assert_eq!(error.parameter(), Some("sketch"));
    assert_eq!(error.elements(), ["n0000003", "p0000001", "p0000002"]);
    assert_eq!(error.elements_total(), 3);
    assert_eq!(
        error.message(),
        "groove g0000001 sweeps sketch k7f2qx3m, which makes no profile (2 refusals)"
    );
    let advice = "end another curve at the point to close the outline, or make the curve construction geometry";
    assert_eq!(
        error.cause(),
        Some(
            format!(
                "point p0000001 ends line n0000003 and no other profile curve, so the profile is open there ({advice}); point p0000002 ends line n0000003 and no other profile curve, so the profile is open there"
            )
            .as_str()
        )
    );
    assert_eq!(
        error.suggestion(),
        Some("fix sketch k7f2qx3m where the cause says, or sweep another sketch")
    );

    // No curves: one refusal of no elements.
    let empty = draw::Draw::new().profile().unwrap_err();
    let error = unprofiled(
        &fid("p0000001"),
        SweepFeature::Pad,
        &draw::feature(),
        &empty,
    );
    assert_eq!(
        error.message(),
        "pad p0000001 sweeps sketch k7f2qx3m, which makes no profile (1 refusal)"
    );
    assert_eq!((error.elements(), error.elements_total()), (&[][..], 0));

    // 51 open lines apart, (0, 2i) to (1, 2i): 102 open ends, the first hundred given and
    // the 2 more counted; 153 elements at fault (51 lines and 102 points), the first
    // hundred by ID listed. The last line and its ends are in the 2 refusals past the
    // hundred alone, and counted.
    let mut d = draw::Draw::new();
    for i in 0..51 {
        let y = 2.0 * f64::from(i);
        let a = d.point(0.0, y);
        let b = d.point(1.0, y);
        d.line(&a, &b);
    }
    let refused = d.profile().unwrap_err();
    assert_eq!((refused.errors().len(), refused.total()), (100, 102));
    let error = unprofiled(
        &fid("p0000001"),
        SweepFeature::Pad,
        &draw::feature(),
        &refused,
    );
    assert_eq!(
        error.message(),
        "pad p0000001 sweeps sketch k7f2qx3m, which makes no profile (102 refusals)"
    );
    // By ID: the 51 lines (n0000003, n0000006, ... n0000153) first, then the points not
    // a multiple of 3, the 49th of which is 3 · 24 + 1 = 73.
    assert_eq!(error.elements().len(), 100);
    assert_eq!(error.elements()[0], "n0000003");
    assert_eq!(error.elements()[50], "n0000153");
    assert_eq!(error.elements()[99], "p0000073");
    assert_eq!(error.elements_total(), 153);
    // Each refusal said, and their one advice once.
    let cause = error.cause().unwrap();
    assert_eq!(cause.matches("so the profile is open there").count(), 100);
    assert_eq!(cause.matches("(end another curve at the point").count(), 1);
    assert!(
        cause.ends_with("so the profile is open there; and 2 more"),
        "{cause}"
    );
}

#[test]
fn a_sweep_of_a_sketch_whose_profile_refusal_is_a_bug_carries_the_bugs_code_and_advice() {
    // A solution missing a point the sketch draws from: the profile's refusal is internal
    // (a bug, never a document's), so the sweep's error is internal, with the advice the
    // codebase gives a bug, not "fix sketch ...". Recompute can't reach it (its solution is
    // read back from the sketch's own translation, every point of the sketch in it), so
    // this is the pure function.
    let mut d = draw::Draw::new();
    let a = d.point(0.0, 0.0);
    let b = d.point(4.0, 0.0);
    let c = d.point(4.0, 3.0);
    d.line(&a, &b);
    d.line(&b, &c);
    d.line(&c, &a);
    let mut solution = d.solution();
    solution
        .points
        .remove(&silico_core::document::ElementId::new(&c).unwrap());
    let refused = d.profile_from(&solution).unwrap_err();
    assert_eq!(refused.errors()[0].code(), ErrorCode::Internal);
    let error = unprofiled(
        &fid("p0000001"),
        SweepFeature::Pad,
        &draw::feature(),
        &refused,
    );
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(error.feature(), Some("p0000001"));
    assert_eq!(
        error.suggestion(),
        Some("this is a bug in Silico, not in the request: report it")
    );
}

#[test]
fn a_cause_gives_each_of_its_different_advices_once() {
    // Rectangles (0, 0)–(4, 1) and (1, −1)–(2, 3), whose sides cross four times, and a lone
    // line (10, 0)–(11, 0), open at both ends: six refusals of two kinds, two open ends
    // and four crossings, each kind with its own advice. A pad of the sketch has one error,
    // its cause every refusal, and each advice exactly once, after the first refusal it
    // follows.
    let mut d = draw::Draw::new();
    d.rectangle(0.0, 0.0, 4.0, 1.0);
    d.rectangle(1.0, -1.0, 2.0, 3.0);
    let a = d.point(10.0, 0.0);
    let b = d.point(11.0, 0.0);
    d.line(&a, &b);
    let refused = d.profile().unwrap_err();
    assert_eq!(refused.total(), 6);
    let error = unprofiled(
        &fid("p0000001"),
        SweepFeature::Pad,
        &draw::feature(),
        &refused,
    );
    let cause = error.cause().unwrap();
    let open = "(end another curve at the point to close the outline, or make the curve construction geometry)";
    let cross = "(move or remove one of them: a profile's curves meet only at the points they share, and each point joins two curves)";
    assert_eq!(cause.matches(open).count(), 1, "{cause}");
    assert_eq!(cause.matches(cross).count(), 1, "{cause}");
    assert_eq!(
        cause.matches("so the profile is open there").count(),
        2,
        "{cause}"
    );
    assert_eq!(cause.matches(" cross").count(), 4, "{cause}");
    // Each advice right after the first refusal it belongs to: the two open ends first, then
    // the four crossings.
    let parts: Vec<&str> = cause.split("; ").collect();
    assert_eq!(parts.len(), 6, "{cause}");
    assert!(parts[0].ends_with(open), "{cause}");
    assert!(
        !parts[1].ends_with(open) && !parts[1].ends_with(cross),
        "{cause}"
    );
    assert!(parts[2].ends_with(cross), "{cause}");
    assert!(parts[3..].iter().all(|p| !p.ends_with(cross)), "{cause}");
    // Each part's kind: the two open ends, then the four crossings.
    for part in &parts[..2] {
        assert!(
            part.starts_with("point p") && part.contains("so the profile is open there"),
            "{part}"
        );
    }
    for part in &parts[2..] {
        assert!(
            part.starts_with("line n") && part.contains(" cross"),
            "{part}"
        );
    }
}
