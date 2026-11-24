//! The pattern's own logic, by hand: which instances it places and in what order, where each
//! goes (from whole nanometres and nanodegrees, worked by hand), its references resolved and
//! refused, and what a step's history says of its copies. Each refusal of section 4.2 that
//! a document of today's types can't reach on the real kernel is held to the design's
//! example byte for byte, read from the design (docs/design/m6.0-patterns.md).

use super::*;
use crate::command::wire::{Layout, ToWire, write};
use crate::document::{Angle, Copies, LinearDirection, OriginPlane, Skipped};
use crate::kernel::{ElementCounts, ElementFate, InputHistory};
use silico_protocol::FaceOrientation;
use silico_protocol::Topology;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn name(text: &str) -> ElementName {
    ElementName::new(text).unwrap()
}

fn mm(value: i64) -> Length {
    Length::from_nm(value * 1_000_000)
}

fn degrees(value: i64) -> Angle {
    Angle::from_ndeg(value * 1_000_000_000)
}

fn direction(
    along: OriginOrName<OriginAxis>,
    distribution: LinearDistribution,
    count: u32,
) -> LinearDirection {
    LinearDirection {
        along,
        reversed: false,
        distribution,
        count,
    }
}

fn x(distribution: LinearDistribution, count: u32) -> LinearDirection {
    direction(OriginOrName::Origin(OriginAxis::X), distribution, count)
}

fn linear(directions: Vec<LinearDirection>, skip: &[&[u32]]) -> Operation {
    Operation::LinearPattern {
        base: id("q0000002"),
        copies: Copies::Features {
            features: vec![id("q0000002")],
        },
        directions,
        skip: skip.iter().map(|s| Skipped(s.to_vec())).collect(),
    }
}

fn polar(distribution: PolarDistribution, count: u32, reversed: bool) -> Operation {
    Operation::PolarPattern {
        base: id("q0000003"),
        copies: Copies::Features {
            features: vec![id("q0000003")],
        },
        axis: OriginOrName::Origin(OriginAxis::Z),
        reversed,
        distribution,
        count,
        skip: Vec::new(),
    }
}

fn texts(instances: &[Instance]) -> Vec<String> {
    instances.iter().map(Instance::to_string).collect()
}

/// The design's text between the headings `from` and `to`, and its JSON blocks.
fn examples(from: &str, to: &str) -> Vec<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/design/m6.0-patterns.md");
    let doc = std::fs::read_to_string(path).unwrap();
    let start = doc.find(from).unwrap();
    let end = start + doc[start..].find(to).unwrap();
    doc[start..end]
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

fn wire(error: &Error) -> String {
    write(&error.to_wire(), Layout::Canonical).unwrap()
}

/// Section 4.2's examples, in order.
fn rebuild_examples() -> Vec<String> {
    examples("### 4.2 The rebuild", "### 4.3 The warning")
}

fn line(start: [f64; 3], end: [f64; 3]) -> Geometry {
    Geometry::Edge(EdgeDescription {
        curve: EdgeCurve::Line,
        length: vec3::norm(vec3::sub(end, start)),
        start,
        end,
        middle: [0.0; 3],
    })
}

fn plane(normal: [f64; 3], location: [f64; 3], orientation: FaceOrientation) -> Geometry {
    Geometry::Face(FaceDescription {
        surface: Surface::Plane { normal, location },
        area: 1.0,
        centroid: location,
        orientation,
    })
}

#[test]
fn a_grids_instances_are_placed_in_skips_order_the_original_and_skips_in_range_left_out() {
    // 3 × 2: (0, 1), (1), (1, 1), (2), (2, 1), as `skip` sorts them, direction 1 first.
    let grid = linear(
        vec![
            x(LinearDistribution::Extent { length: mm(80) }, 3),
            x(LinearDistribution::Extent { length: mm(40) }, 2),
        ],
        &[],
    );
    assert_eq!(texts(&placed(&grid)), ["0,1", "1", "1,1", "2", "2,1"]);
    // [1, 1] skipped; [3], past the count, takes nothing away (Main, 2026-11-17).
    let skipped = linear(
        vec![
            x(LinearDistribution::Extent { length: mm(80) }, 3),
            x(LinearDistribution::Extent { length: mm(40) }, 2),
        ],
        &[&[1, 1], &[3]],
    );
    assert_eq!(texts(&placed(&skipped)), ["0,1", "1", "2", "2,1"]);
    // Every copy skipped, or a count of 1: the identity, nothing placed.
    let all = linear(
        vec![x(LinearDistribution::Spacing { length: mm(5) }, 2)],
        &[&[1]],
    );
    assert!(placed(&all).is_empty());
    let one = linear(
        vec![x(LinearDistribution::Spacing { length: mm(5) }, 1)],
        &[],
    );
    assert!(placed(&one).is_empty());
    // A polar pattern's 1 to count − 1; a mirror's one copy, 1.
    assert_eq!(
        texts(&placed(&polar(
            PolarDistribution::Extent {
                angle: degrees(360)
            },
            4,
            false
        ))),
        ["1", "2", "3"]
    );
    let mirror = Operation::Mirror {
        base: id("q0000001"),
        copies: Copies::Solid,
        plane: OriginOrName::Origin(OriginPlane::Yz),
    };
    assert_eq!(texts(&placed(&mirror)), ["1"]);
}

#[test]
fn a_linear_instance_moves_by_its_steps_along_each_direction_from_whole_nanometres() {
    // The plate's corner holes: 80 over 3 and 40 over 2, a step of 40 each way, so (2, 1)
    // moves by (80, 40, 0), (0, 1) by (0, 40, 0).
    let grid = linear(
        vec![
            x(LinearDistribution::Extent { length: mm(80) }, 3),
            direction(
                OriginOrName::Origin(OriginAxis::Y),
                LinearDistribution::Extent { length: mm(40) },
                2,
            ),
        ],
        &[],
    );
    let resolved = Resolved::Linear(vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    let instances = placed(&grid);
    let placements = placements(&id("t0000001"), &grid, &resolved, &instances).unwrap();
    let offsets: Vec<[f64; 3]> = placements
        .iter()
        .map(|p| match p {
            Placement::Translation { offset } => *offset,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        offsets,
        [
            [0.0, 40.0, 0.0],
            [40.0, 0.0, 0.0],
            [40.0, 40.0, 0.0],
            [80.0, 0.0, 0.0],
            [80.0, 40.0, 0.0],
        ]
    );
    // An extent of 0.1 over 4: each instance i · 100000 nm / 3, divided once, so the third
    // is 0.1 exactly, where adding a step three times gives 0.30000000000000004 / 3's
    // neighbour.
    assert_eq!(
        linear_step(
            LinearDistribution::Extent {
                length: Length::from_nm(100_000)
            },
            4,
            3
        ),
        0.1
    );
    assert_eq!(
        linear_step(
            LinearDistribution::Extent {
                length: Length::from_nm(100_000)
            },
            4,
            1
        ),
        100_000.0 / 3e6
    );
    // A spacing of 8: instance 5 at 40.
    assert_eq!(
        linear_step(LinearDistribution::Spacing { length: mm(8) }, 6, 5),
        40.0
    );
}

#[test]
fn a_polar_instance_turns_by_a_whole_turn_over_count_or_an_extent_over_count_less_one() {
    // 360° ÷ 4 at 0, 90, 180, 270; 180° ÷ 4 at 0, 60, 120, 180; 30° spacing at 30k;
    // 359.999999999° is no whole turn, so ÷ 3 (decision 5).
    let at = |distribution, count| -> Vec<f64> {
        (1..count)
            .map(|k| polar_degrees(distribution, count, k))
            .collect()
    };
    assert_eq!(
        at(
            PolarDistribution::Extent {
                angle: degrees(360)
            },
            4
        ),
        [90.0, 180.0, 270.0]
    );
    assert_eq!(
        at(
            PolarDistribution::Extent {
                angle: degrees(180)
            },
            4
        ),
        [60.0, 120.0, 180.0]
    );
    assert_eq!(
        at(PolarDistribution::Spacing { angle: degrees(30) }, 5),
        [30.0, 60.0, 90.0, 120.0]
    );
    let short = PolarDistribution::Extent {
        angle: Angle::from_ndeg(359_999_999_999),
    };
    assert_eq!(at(short, 4)[2], 359.999999999);
    assert_eq!(at(short, 4)[0], 359.999999999 / 3.0);
    // Each turn in radians at the boundary, about the axis, which `reversed` turns round.
    let pattern = polar(
        PolarDistribution::Extent {
            angle: degrees(360),
        },
        4,
        true,
    );
    let resolved = resolve(&id("t0000002"), &pattern, &mut |_| unreachable!()).unwrap();
    assert_eq!(
        resolved,
        Resolved::Polar(Axis {
            origin: [0.0; 3],
            direction: [0.0, 0.0, -1.0]
        })
    );
    let placements = placements(&id("t0000002"), &pattern, &resolved, &placed(&pattern)).unwrap();
    let Placement::Rotation { angle, .. } = placements[1] else {
        panic!("{placements:?}");
    };
    assert_eq!(angle, silico_protocol::radians(180.0));
}

#[test]
fn a_named_direction_is_its_edge_from_start_to_end_or_its_faces_outward_normal_reversed_by_reversed()
 {
    let along = name("p0000001.pad.Edge(M s0000001.sketch.Edge.na000001)");
    let face = name("p0000001.pad.Face(G s0000001.sketch.Edge.na000001)");
    let pattern = linear(
        vec![
            LinearDirection {
                along: OriginOrName::Name(along.clone()),
                reversed: false,
                distribution: LinearDistribution::Spacing { length: mm(8) },
                count: 6,
            },
            LinearDirection {
                along: OriginOrName::Name(face.clone()),
                reversed: true,
                distribution: LinearDistribution::Spacing { length: mm(8) },
                count: 3,
            },
        ],
        &[],
    );
    // The front bottom edge from (−50, −30, 0) to (50, −30, 0); the front face's outward
    // normal −Y, reversed into the plate, +Y (section 2.3).
    let resolved = resolve(&id("t0000003"), &pattern, &mut |n| {
        Ok(if *n == along {
            line([-50.0, -30.0, 0.0], [50.0, -30.0, 0.0])
        } else {
            plane(
                [0.0, -1.0, 0.0],
                [0.0, -30.0, 0.0],
                FaceOrientation::Reversed,
            )
        })
    })
    .unwrap();
    assert_eq!(
        resolved,
        Resolved::Linear(vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]])
    );
}

#[test]
fn a_circle_as_a_direction_is_refused_in_the_designs_words() {
    let rim = name(
        "q0000002.pocket.Edge(G p0000001.pad.Face(G s0000001.sketch.Face1), G q0000002.pocket.Face(G s0000003.sketch.Edge.cc000001))",
    );
    let pattern = linear(
        vec![direction(
            OriginOrName::Name(rim),
            LinearDistribution::Extent { length: mm(80) },
            3,
        )],
        &[],
    );
    let error = resolve(&id("t0000001"), &pattern, &mut |_| {
        Ok(Geometry::Edge(EdgeDescription {
            curve: EdgeCurve::Circle {
                axis: Axis {
                    origin: [40.0, 20.0, 5.0],
                    direction: [0.0, 0.0, 1.0],
                },
                radius: 2.75,
            },
            length: 5.5 * PI,
            start: [42.75, 20.0, 5.0],
            end: [42.75, 20.0, 5.0],
            middle: [37.25, 20.0, 5.0],
        }))
    })
    .unwrap_err();
    assert_eq!(wire(&error), rebuild_examples()[1]);
}

#[test]
fn a_face_with_no_outside_is_refused_as_a_direction_in_the_designs_words() {
    // Fed the orientation as the kernel reports it: an internal face's, and an external
    // one's alike. A forward or reversed face of the same plane is taken.
    let membrane = name("b0000002.box.Face1");
    let pattern = |id_: &str| Operation::LinearPattern {
        base: id("j0000001"),
        copies: Copies::Solid,
        directions: vec![direction(
            OriginOrName::Name(name(id_)),
            LinearDistribution::Spacing { length: mm(30) },
            2,
        )],
        skip: Vec::new(),
    };
    for orientation in [FaceOrientation::Internal, FaceOrientation::External] {
        let error = resolve(&id("t0000010"), &pattern(membrane.as_str()), &mut |_| {
            Ok(plane([-1.0, 0.0, 0.0], [0.0; 3], orientation))
        })
        .unwrap_err();
        assert_eq!(wire(&error), rebuild_examples()[2], "{orientation:?}");
    }
    for orientation in [FaceOrientation::Forward, FaceOrientation::Reversed] {
        assert_eq!(
            resolve(&id("t0000010"), &pattern(membrane.as_str()), &mut |_| {
                Ok(plane([-1.0, 0.0, 0.0], [0.0; 3], orientation))
            }),
            Ok(Resolved::Linear(vec![[-1.0, 0.0, 0.0]]))
        );
    }
}

#[test]
fn two_parallel_directions_either_way_are_refused_in_the_designs_words() {
    let edge = name("p0000001.pad.Edge(M s0000001.sketch.Edge.na000001)");
    let pattern = |reversed: bool| {
        linear(
            vec![
                x(LinearDistribution::Extent { length: mm(80) }, 3),
                LinearDirection {
                    along: OriginOrName::Name(edge.clone()),
                    reversed,
                    distribution: LinearDistribution::Extent { length: mm(40) },
                    count: 2,
                },
            ],
            &[],
        )
    };
    // Along x, and against it.
    for (start, end) in [
        ([-50.0, -30.0, 0.0], [50.0, -30.0, 0.0]),
        ([50.0, -30.0, 0.0], [-50.0, -30.0, 0.0]),
    ] {
        for reversed in [false, true] {
            let error = resolve(&id("t0000001"), &pattern(reversed), &mut |_| {
                Ok(line(start, end))
            })
            .unwrap_err();
            assert_eq!(wire(&error), rebuild_examples()[3]);
        }
    }
    // 2e-12 radians apart is not parallel; 5e-13 is.
    assert!(!parallel(
        [1.0, 0.0, 0.0],
        [libm::cos(2e-12), libm::sin(2e-12), 0.0]
    ));
    assert!(parallel(
        [1.0, 0.0, 0.0],
        [libm::cos(5e-13), libm::sin(5e-13), 0.0]
    ));
    assert!(parallel(
        [1.0, 0.0, 0.0],
        [-libm::cos(5e-13), libm::sin(5e-13), 0.0]
    ));
}

#[test]
fn an_instance_two_directions_place_past_a_kilometre_is_refused_in_the_designs_words() {
    // 500 instances 2000 apart along x, and 2 along an edge at 45° in z = 0, 5000 apart:
    // instance (499, 1) moves 998000 + 5000 · cos 45° along x (section 4.2).
    let edge = name("p0000009.pad.Edge(M s0000009.sketch.Edge.na000009)");
    let pattern = linear(
        vec![
            x(LinearDistribution::Spacing { length: mm(2000) }, 500),
            direction(
                OriginOrName::Name(edge),
                LinearDistribution::Spacing { length: mm(5000) },
                2,
            ),
        ],
        &[],
    );
    let resolved = resolve(&id("t0000001"), &pattern, &mut |_| {
        Ok(line([0.0; 3], [10.0, 10.0, 0.0]))
    })
    .unwrap();
    let error = placements(&id("t0000001"), &pattern, &resolved, &placed(&pattern)).unwrap_err();
    assert_eq!(wire(&error), rebuild_examples()[4]);
}

#[test]
fn a_mirror_plane_on_a_face_passes_through_the_origin_projected_onto_it() {
    // A face at y = 30, its plane's own location (7, 30, 2): o = n(n·p) = (0, 30, 0).
    let back = name("p0000001.pad.Face(G s0000001.sketch.Edge.na000003)");
    let mirror = Operation::Mirror {
        base: id("c0000001"),
        copies: Copies::Solid,
        plane: OriginOrName::Name(back),
    };
    let resolved = resolve(&id("m0000002"), &mirror, &mut |_| {
        Ok(plane(
            [0.0, 1.0, 0.0],
            [7.0, 30.0, 2.0],
            FaceOrientation::Forward,
        ))
    })
    .unwrap();
    assert_eq!(
        resolved,
        Resolved::Mirror {
            origin: [0.0, 30.0, 0.0],
            normal: [0.0, 1.0, 0.0]
        }
    );
    // A curved face is no plane.
    let error = resolve(&id("m0000002"), &mirror, &mut |_| {
        Ok(Geometry::Face(FaceDescription {
            surface: Surface::Cylinder {
                axis: Axis {
                    origin: [0.0; 3],
                    direction: [0.0, 0.0, 1.0],
                },
                radius: 2.0,
            },
            area: 1.0,
            centroid: [0.0; 3],
            orientation: FaceOrientation::Forward,
        }))
    })
    .unwrap_err();
    assert_eq!(error.parameter(), Some("plane"));
    assert!(
        error
            .message()
            .ends_with(", is a cylinder face: a mirror plane is a planar face"),
        "{}",
        error.message()
    );
}

/// A history of a base of `base` faces, kept as 1 to `base`, then each copy of `faces`
/// faces, kept or gone as `fates` says (the next numbers after the base's), with each
/// result solid's faces.
fn history(base: u32, copies: &[Vec<bool>], solids: Vec<Vec<u32>>) -> History {
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        modified: Vec::new(),
        generated: Vec::new(),
    };
    let mut next = base;
    let mut inputs = vec![InputHistory {
        faces: (1..=base).map(kept).collect(),
        edges: Vec::new(),
        vertices: Vec::new(),
    }];
    for copy in copies {
        inputs.push(InputHistory {
            faces: copy
                .iter()
                .map(|&there| {
                    if there {
                        next += 1;
                        kept(next)
                    } else {
                        ElementFate::default()
                    }
                })
                .collect(),
            edges: Vec::new(),
            vertices: Vec::new(),
        });
    }
    History {
        inputs,
        result: ElementCounts {
            faces: next,
            solids: solids.len() as u32,
            ..ElementCounts::default()
        },
        topology: Topology {
            solid_faces: solids,
            ..Topology::default()
        },
        ..History::default()
    }
}

#[test]
fn a_steps_history_names_the_copies_off_the_base_and_those_that_changed_nothing() {
    // A base of 6 faces; copies 1 to 4 of 3 faces. Copy 2's faces are all gone (it changed
    // nothing); copies 3 and 4 lie in solids of their own.
    let instances: Vec<Instance> = (1..=4).map(|k| Instance::new(&[k]).unwrap()).collect();
    let all = vec![true; 3];
    let h = history(
        6,
        &[all.clone(), vec![false; 3], all.clone(), all.clone()],
        vec![(1..=9).collect(), vec![10, 11, 12], vec![13, 14, 15]],
    );
    let verdict = verdict(&h, &instances, None, CopiesOp::Fuse);
    assert_eq!(texts(&verdict.off_base), ["3", "4"]);
    assert_eq!(texts(&verdict.changed_nothing), ["2"]);
    assert_eq!(verdict.base_solids, 1);
    // Refused naming exactly 3 and 4, in the design's words (section 4.2's example's).
    let error = solids_refused(
        &id("t0000005"),
        "linear_pattern",
        &id("p0000005"),
        &verdict,
        false,
        None,
    )
    .unwrap();
    assert_eq!(wire(&error), rebuild_examples()[5]);
    // A base whose faces lie in two solids names no instance.
    let split = history(
        6,
        std::slice::from_ref(&all),
        vec![vec![1, 2, 3, 7], vec![4, 5, 6, 8, 9]],
    );
    let verdict = super::verdict(&split, &instances[..1], None, CopiesOp::Fuse);
    assert!(verdict.off_base.is_empty());
    assert_eq!(verdict.base_solids, 2);
    // A copy that generates a face is in the result, though none of its faces is kept.
    let mut generating = history(6, &[vec![false; 3]], vec![(1..=6).collect()]);
    generating.inputs[1].faces[0]
        .generated
        .push(silico_protocol::Reported {
            element: crate::kernel::ElementRef {
                kind: ElementKind::Edge,
                index: 1,
            },
            position: 1,
        });
    assert!(
        super::verdict(&generating, &instances[..1], None, CopiesOp::Fuse)
            .changed_nothing
            .is_empty()
    );
}

#[test]
fn a_base_cut_in_pieces_or_left_in_pieces_is_refused_naming_no_instance() {
    let instances: Vec<Instance> = (1..=2).map(|k| Instance::new(&[k]).unwrap()).collect();
    let all = vec![true; 3];
    let h = history(
        6,
        &[all.clone(), all.clone()],
        vec![vec![1, 2, 7], vec![3, 4, 8, 9], vec![5, 6, 10, 11, 12]],
    );
    let cut = verdict(&h, &instances, None, CopiesOp::Cut);
    let error = solids_refused(
        &id("t0000006"),
        "polar_pattern",
        &id("q0000006"),
        &cut,
        false,
        None,
    )
    .unwrap();
    assert_eq!(wire(&error), rebuild_examples()[6]);
    // A fuse that leaves its base in pieces: the base's faces in 2 of 3 solids; the cause
    // says the box and its fuzzy value, a millionth of it.
    let h = history(
        6,
        &[all.clone(), all],
        vec![vec![1, 2, 3, 7], vec![4, 5, 6, 8, 9], vec![10, 11, 12]],
    );
    let fused = verdict(&h, &instances, None, CopiesOp::Fuse);
    let error = solids_refused(
        &id("t0000007"),
        "linear_pattern",
        &id("p0000012"),
        &fused,
        false,
        Some([[-10.0, -10.0, 0.0], [10.0, 999_990.0, 7.0]]),
    )
    .unwrap();
    assert_eq!(
        error.message(),
        "linear_pattern t0000007 leaves its base p0000012 in pieces: the base's faces lie in 2 of the result's 3 solids, so no instance is named"
    );
    assert_eq!(
        error.cause(),
        Some(
            "the copies make the boolean's box about 1000000 mm across, so its fuzzy value is about 1 mm, a millionth of it: OCCT takes what's closer than that as touching"
        )
    );
    assert!(error.elements().is_empty());
    // One solid: nothing refused.
    let one = verdict(
        &history(6, &[vec![true; 3]], vec![(1..=9).collect()]),
        &instances[..1],
        None,
        CopiesOp::Fuse,
    );
    assert!(
        solids_refused(
            &id("t0000005"),
            "linear_pattern",
            &id("b0000001"),
            &one,
            false,
            None
        )
        .is_none()
    );
}

#[test]
fn the_warnings_words_name_each_instance_and_each_original_where_there_are_several() {
    let i = |k: u32| Instance::new(&[k]).unwrap();
    let three = [i(17), i(18), i(19)];
    assert_eq!(
        changed_nothing_message(
            &id("t0000009"),
            "linear_pattern",
            &[(Some(&id("q0000009")), false, &three)]
        )
        .unwrap(),
        "linear_pattern t0000009: the copies of instances 17, 18, and 19 remove nothing (no face of theirs is in the result)"
    );
    let one = [i(2)];
    assert_eq!(
        changed_nothing_message(&id("t0000009"), "mirror", &[(None, true, &one)]).unwrap(),
        "mirror t0000009: the copy of instance 2 adds nothing (none of its faces is in the result)"
    );
    let none: [Instance; 0] = [];
    assert_eq!(
        changed_nothing_message(
            &id("t0000009"),
            "linear_pattern",
            &[
                (Some(&id("p0000001")), true, &one),
                (Some(&id("q0000002")), false, &none),
                (Some(&id("q0000003")), false, &three)
            ]
        )
        .unwrap(),
        "linear_pattern t0000009: the copy of p0000001's instance 2 adds nothing (none of its faces is in the result); the copies of q0000003's instances 17, 18, and 19 remove nothing (no face of theirs is in the result)"
    );
    assert!(changed_nothing_message(&id("t0000009"), "mirror", &[(None, true, &none)]).is_none());
    // Past ten, the rest counted.
    let many: Vec<Instance> = (1..=12).map(i).collect();
    assert!(
        changed_nothing_message(&id("t0000009"), "linear_pattern", &[(None, false, &many)])
            .unwrap()
            .contains("instances 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, and 2 more remove nothing")
    );
}

#[test]
fn the_values_line_holds_each_integer_and_skip_as_written() {
    let grid = linear(
        vec![
            x(LinearDistribution::Extent { length: mm(80) }, 3),
            LinearDirection {
                along: OriginOrName::Origin(OriginAxis::Y),
                reversed: true,
                distribution: LinearDistribution::Spacing {
                    length: Length::from_nm(1),
                },
                count: 2,
            },
        ],
        &[&[1, 1], &[3]],
    );
    assert_eq!(
        values(&grid),
        "direction false extent 80000000 3 direction true spacing 1 2 skip [1,1] [3]"
    );
    assert_eq!(
        values(&polar(
            PolarDistribution::Spacing { angle: degrees(30) },
            5,
            true
        )),
        "turn true spacing 30000000000 5 skip"
    );
    assert_eq!(
        references(&grid),
        [
            ("directions.0.along".to_owned(), "x"),
            ("directions.1.along".to_owned(), "y")
        ]
    );
}

#[test]
fn a_polar_axis_on_a_named_edge_runs_from_its_start_to_its_end_and_a_circle_along_its_axis() {
    // Decision 3: a straight edge's axis is its line from its start to its end, as describe
    // gives them, so the same edge described the other way turns the other way; a circle's,
    // its circle's axis through its centre. `reversed` turns either round.
    let edge = name("p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000002)");
    let about = |reversed: bool| Operation::PolarPattern {
        base: id("p0000001"),
        copies: Copies::Solid,
        axis: OriginOrName::Name(edge.clone()),
        reversed,
        distribution: PolarDistribution::Extent { angle: degrees(90) },
        count: 2,
        skip: Vec::new(),
    };
    let up = line([10.0, 0.0, 0.0], [10.0, 0.0, 10.0]);
    let down = line([10.0, 0.0, 10.0], [10.0, 0.0, 0.0]);
    for (geometry, reversed, origin, direction) in [
        (&up, false, [10.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        (&down, false, [10.0, 0.0, 10.0], [0.0, 0.0, -1.0]),
        (&up, true, [10.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ] {
        let resolved = resolve(&id("t0000002"), &about(reversed), &mut |_| {
            Ok(geometry.clone())
        });
        assert_eq!(
            resolved,
            Ok(Resolved::Polar(Axis { origin, direction })),
            "{geometry:?} {reversed}"
        );
    }
    let rim = Geometry::Edge(EdgeDescription {
        curve: EdgeCurve::Circle {
            axis: Axis {
                origin: [3.0, 4.0, 5.0],
                direction: [0.0, -1.0, 0.0],
            },
            radius: 2.0,
        },
        length: 4.0 * PI,
        start: [5.0, 4.0, 5.0],
        end: [5.0, 4.0, 5.0],
        middle: [1.0, 4.0, 5.0],
    });
    assert_eq!(
        resolve(&id("t0000002"), &about(false), &mut |_| Ok(rim.clone())),
        Ok(Resolved::Polar(Axis {
            origin: [3.0, 4.0, 5.0],
            direction: [0.0, -1.0, 0.0]
        }))
    );
}

#[test]
fn copies_off_the_base_of_several_originals_are_named_with_their_originals() {
    // Decision 4 of section 12: with several originals, the message says whose copies.
    let instances: Vec<Instance> = (1..=2).map(|k| Instance::new(&[k]).unwrap()).collect();
    let all = vec![true; 3];
    let h = history(
        6,
        &[all.clone(), all],
        vec![(1..=9).collect(), vec![10, 11, 12]],
    );
    let verdict = verdict(&h, &instances, Some(id("z0000001")), CopiesOp::Fuse);
    let several = solids_refused(
        &id("t0000005"),
        "linear_pattern",
        &id("p0000005"),
        &verdict,
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        several.message(),
        "linear_pattern t0000005 makes 2 solids: the copy of z0000001's instance 2 doesn't touch the base, or meets it only along an edge or at a point"
    );
    assert_eq!(several.elements(), ["2"]);
    // One original: whose isn't said.
    let one = solids_refused(
        &id("t0000005"),
        "linear_pattern",
        &id("p0000005"),
        &verdict,
        false,
        None,
    )
    .unwrap();
    assert!(
        one.message()
            .contains(": the copy of instance 2 doesn't touch"),
        "{}",
        one.message()
    );
}
