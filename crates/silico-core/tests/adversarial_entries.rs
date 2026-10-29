//! break-it: M4.4's `elements` and `query` (`silico_core::command::elements`), on models
//! whose descriptions a worker at fault might send (`FakeKernel::answer_instead`): normals
//! with `-0`, ties of F1's table in bits, cones at their limits (a half-angle of 0, of `-0`,
//! all but a right angle, a radius of 0), degenerated edges, values past what a length
//! holds; `facing` at 0 and 180 degrees, opposite and orthogonal normals, and directions
//! tiny, huge, NaN, and infinite; ties exactly at 1,000 and 1,001; cursors that aren't the
//! listing's; more ranges than there are measures; types, ranges of measures some faces
//! haven't, edges' ends, and names nothing in a history gives; and sessions not open. Every
//! expected value is by hand from the values forged.

use super::example;
use super::listing::{BOX, TWO_BODIES, answer, codes, fixture, model_of};
use super::model;

use std::f64::consts::FRAC_1_SQRT_2;
use std::sync::Arc;

use example::EXAMPLE;
use model::{checked, id, published};
use silico_core::ErrorCode;
use silico_core::command::ModelChoice;
use silico_core::command::Revision;
use silico_core::command::elements::{
    AxisToken, Cursor, ENTRIES_AT_MOST, ElementType, Entry, Facing, MEASURES_AT_MOST, MeasureName,
    Order, Quantity, Query, Range, entries, list, query,
};
use silico_core::document::{Angle, ElementName, Length, Measure};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Axis, BooleanOp, BuiltData, ElementCounts, ElementFate, ElementKind, History, InputHistory, Op,
    Topology,
};
use silico_core::recompute::{Model, Recompute};
use silico_protocol::{Description, EdgeCurve, EdgeDescription, FaceDescription, Reply, Surface};

fn plane(normal: [f64; 3]) -> FaceDescription {
    FaceDescription {
        surface: Surface::Plane { normal },
        area: 1.0,
        centroid: [0.0; 3],
    }
}

fn cone(origin: [f64; 3], direction: [f64; 3], radius: f64, half_angle: f64) -> FaceDescription {
    FaceDescription {
        surface: Surface::Cone {
            axis: Axis { origin, direction },
            radius,
            half_angle,
        },
        area: 1.0,
        centroid: [0.0; 3],
    }
}

fn line(n: u32) -> EdgeDescription {
    let x = f64::from(n);
    EdgeDescription {
        curve: EdgeCurve::Line,
        length: 1.0,
        start: [x, 0.0, 0.0],
        end: [x, 1.0, 0.0],
        middle: [x, 0.5, 0.0],
    }
}

/// The box `BOX`, rebuilt on a fake kernel that describes its faces as `faces` and its
/// edges as `edges` (or as the fake does, for none).
fn forged_box(faces: Vec<FaceDescription>, edges: Option<Vec<EdgeDescription>>) -> Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().answer_instead(move |op| match op {
        // Every element at once: the faces and edges given, the rest the fake's.
        Op::DescribeAll { shape } => {
            let fake = FakeKernel::described(shape)?;
            let edges = edges.clone().unwrap_or(fake.edges);
            Some(Reply::DescribedAll(Description {
                faces: faces.clone(),
                edges,
                vertices: fake.vertices,
            }))
        }
        Op::Describe {
            kind: ElementKind::Face,
            first,
            count,
            ..
        } => {
            let from = (*first - 1) as usize;
            Some(Reply::Described {
                total: faces.len() as u32,
                description: Description {
                    faces: faces[from..from + *count as usize].to_vec(),
                    edges: Vec::new(),
                    vertices: Vec::new(),
                },
            })
        }
        Op::Describe {
            kind: ElementKind::Edge,
            first,
            count,
            ..
        } => edges.as_ref().map(|edges| {
            let from = (*first - 1) as usize;
            Reply::Described {
                total: edges.len() as u32,
                description: Description {
                    faces: Vec::new(),
                    edges: edges[from..from + *count as usize].to_vec(),
                    vertices: Vec::new(),
                },
            }
        }),
        _ => None,
    });
    published(&engine, checked(BOX))
}

/// The box's entry named `bbbbbbbb.box.<element>`.
fn named<'a>(entries: &'a [Entry], element: &str) -> &'a Entry {
    let name = format!("bbbbbbbb.box.{element}");
    entries
        .iter()
        .find(|e| e.name.to_string() == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn deg(degrees: i64) -> Angle {
    Angle::from_ndeg(degrees * 1_000_000_000)
}

// ---------------------------------------------------------------------------------------
// Entries

#[test]
fn a_normal_and_an_axis_with_negative_zeros_are_written_with_positive_ones() {
    // A plane's normal (-0, -0, 1) and a cylinder's axis (-0, -0, -1), as OCCT gives a
    // reversed face's: written (0, 0, 1) and (0, 0, -1), each zero +0 in its bits.
    let mut faces = vec![plane([-0.0, -0.0, 1.0])];
    faces.push(FaceDescription {
        surface: Surface::Cylinder {
            axis: Axis {
                origin: [-0.0, -0.0, -0.0],
                direction: [-0.0, -0.0, -1.0],
            },
            radius: 2.5,
        },
        area: 1.0,
        centroid: [-0.0, -0.0, -0.0],
    });
    faces.extend((0..4).map(|_| plane([1.0, 0.0, 0.0])));
    let model = forged_box(faces, None);
    let entries = entries(&model, &id("bbbbbbbb")).unwrap();
    let top = named(entries, "Face1");
    let normal = top.normal.unwrap();
    assert_eq!(normal.map(f64::to_bits), [0.0, 0.0, 1.0].map(f64::to_bits));
    assert_eq!(top.default_x, Some(AxisToken::PlusX));
    let side = named(entries, "Face2");
    let axis = side.axis.clone().unwrap();
    assert_eq!(
        axis.direction.map(f64::to_bits),
        [0.0, 0.0, -1.0].map(f64::to_bits)
    );
    assert_eq!(axis.point, [Length::from_nm(0); 3]);
    assert_eq!(side.radius, Some(Length::from_nm(2_500_000)));
    assert_eq!(side.diameter, Some(Length::from_nm(5_000_000)));
}

#[test]
fn f1s_table_breaks_ties_x_then_y_then_z_on_the_normals_bits() {
    // docs/ports/attacher.md's 45° ties, by hand: X = Y gives +y (n·x > 0); X = Z +y; Y = Z
    // -x (n·y > 0) or +x (n·y < 0); all three +y. And one ulp either way decides it: the
    // larger component wins, as the normal is written.
    let h = FRAC_1_SQRT_2;
    let t = 1.0 / 3.0f64.sqrt();
    for (n, want) in [
        ([h, h, 0.0], AxisToken::PlusY),
        ([h, -h, 0.0], AxisToken::PlusY),
        ([-h, h, 0.0], AxisToken::MinusY),
        ([h, 0.0, h], AxisToken::PlusY),
        ([-h, 0.0, h], AxisToken::MinusY),
        ([0.0, h, h], AxisToken::MinusX),
        ([0.0, -h, h], AxisToken::PlusX),
        ([0.0, h, -h], AxisToken::MinusX),
        ([t, t, t], AxisToken::PlusY),
        ([-t, -t, -t], AxisToken::MinusY),
        // An ulp: y over x, then z over x, then z over y.
        ([h, h.next_up(), 0.0], AxisToken::MinusX),
        ([h.next_down(), h, 0.0], AxisToken::MinusX),
        ([h, 0.0, h.next_up()], AxisToken::PlusX),
        ([0.0, -h, -h.next_up()], AxisToken::PlusX),
        // -0 as 0: nearest x, its sign the only one there is.
        ([-1.0, -0.0, -0.0], AxisToken::MinusY),
        ([1.0, -0.0, 0.0], AxisToken::PlusY),
        ([-0.0, -1.0, 0.0], AxisToken::PlusX),
        ([-0.0, -0.0, -1.0], AxisToken::PlusX),
    ] {
        assert_eq!(AxisToken::default_x(n), want, "{n:?}");
    }
    // Through an entry: each face's default_x is its written normal's.
    let normals = [
        [h, h, -0.0],
        [h, 0.0, h],
        [0.0, -h, h],
        [h, h.next_up(), 0.0],
        [t, t, t],
        [0.0, 0.0, -1.0],
    ];
    let model = forged_box(normals.iter().copied().map(plane).collect(), None);
    let entries = entries(&model, &id("bbbbbbbb")).unwrap();
    for (i, want) in [
        AxisToken::PlusY,
        AxisToken::PlusY,
        AxisToken::PlusX,
        AxisToken::MinusX,
        AxisToken::PlusY,
        AxisToken::PlusX,
    ]
    .into_iter()
    .enumerate()
    {
        let face = named(entries, &format!("Face{}", i + 1));
        assert_eq!(face.default_x, Some(want), "Face{}", i + 1);
        assert_eq!(
            face.default_x,
            Some(AxisToken::default_x(face.normal.unwrap()))
        );
    }
}

#[test]
fn a_cone_at_its_limits_has_its_half_angle_and_an_apex_only_where_a_length_holds_it() {
    let up = [0.0, 0.0, 1.0];
    let down = [0.0, 0.0, -1.0];
    // atan(4 / 6), by hand 33.690067525979785 degrees.
    let narrowing = -libm::atan(4.0 / 6.0);
    let faces = vec![
        // A half-angle of 0, and of -0: no apex (r / tan 0 is infinite), the angle 0.
        cone([0.0; 3], up, 2.0, 0.0),
        cone([0.0; 3], up, 2.0, -0.0),
        // A radius of 0 at its axis's point: its apex is that point.
        cone([1.0, 2.0, 3.0], up, 0.0, 0.5),
        // Narrowing along -z from r 4 at the origin over 6: apex at z = -6.
        cone([0.0; 3], down, 4.0, narrowing),
        // All but a right angle: its apex all but its axis's point, the angle 90 as
        // written (rounded once, to 9 places).
        cone(
            [7.0, 0.0, 0.0],
            up,
            1.0,
            std::f64::consts::FRAC_PI_2 - 1e-12,
        ),
        // All but a cylinder: r / tan(1e-20) = 1e20 mm, past what a length holds.
        cone([0.0; 3], up, 1.0, 1e-20),
    ];
    let model = forged_box(faces, None);
    let entries = entries(&model, &id("bbbbbbbb")).unwrap();
    let face = |n: u32| named(entries, &format!("Face{n}"));
    for n in [1, 2] {
        assert_eq!(face(n).half_angle, Some(Angle::from_ndeg(0)), "Face{n}");
        assert_eq!(face(n).apex, None, "Face{n}");
        assert_eq!(face(n).r#type, ElementType::Cone);
        assert_eq!((face(n).radius, face(n).diameter), (None, None));
    }
    assert_eq!(
        face(3).apex,
        Some([1, 2, 3].map(|mm| Length::from_nm(mm * 1_000_000)))
    );
    assert_eq!(face(3).half_angle, Some(Angle::from_ndeg(28_647_889_757)));
    assert_eq!(face(4).half_angle, Some(Angle::from_ndeg(-33_690_067_526)));
    let apex = face(4).apex.unwrap();
    assert_eq!(apex[0..2], [Length::from_nm(0); 2]);
    assert!(
        (apex[2].nm() + 6_000_000).abs() <= 1,
        "{:?}, not (0, 0, -6)",
        apex
    );
    assert_eq!(face(5).half_angle, Some(deg(90)));
    assert_eq!(
        face(5).apex,
        Some([
            Length::from_nm(7_000_000),
            Length::from_nm(0),
            Length::from_nm(0)
        ])
    );
    assert_eq!(face(6).apex, None);
    assert_eq!(face(6).half_angle, Some(Angle::from_ndeg(0)));
}

#[test]
fn a_degenerated_edge_is_other_0_long_not_straight_its_ends_its_point() {
    let mut edges = vec![EdgeDescription {
        curve: EdgeCurve::Degenerated,
        length: 0.0,
        start: [1.0, 2.0, 3.0],
        end: [1.0, 2.0, 3.0],
        middle: [1.0, 2.0, 3.0],
    }];
    edges.extend((2..=12).map(line));
    let faces = (0..6).map(|_| plane([0.0, 0.0, 1.0])).collect();
    let model = forged_box(faces, Some(edges));
    let entries = entries(&model, &id("bbbbbbbb")).unwrap();
    let pole = named(entries, "Edge1");
    let at = Some([1, 2, 3].map(|mm| Length::from_nm(mm * 1_000_000)));
    assert_eq!(pole.r#type, ElementType::Other);
    assert_eq!(pole.length, Some(Length::from_nm(0)));
    assert_eq!(pole.straight, Some(false));
    assert_eq!((pole.start, pole.end, pole.midpoint), (at, at, None));
    assert_eq!(
        (pole.radius, pole.diameter, pole.axis.clone()),
        (None, None, None)
    );
    // A line beside it is straight, with its middle.
    let edge = named(entries, "Edge2");
    assert_eq!(edge.straight, Some(true));
    assert_eq!(
        edge.midpoint,
        Some([
            Length::from_nm(2_000_000),
            Length::from_nm(500_000),
            Length::from_nm(0)
        ])
    );
}

#[test]
fn lengths_round_once_to_the_nanometre_and_a_tiny_negative_is_zero() {
    // A centre of area at (-4e-10, 1.5e-6, 2.4999999e-6) mm: 0, 2, and 2 nm (1.5e-6 is a
    // little over in binary, 1.50000000000000004e-6).
    let mut faces = vec![FaceDescription {
        surface: Surface::Other,
        area: 0.0000004,
        centroid: [-4e-10, 1.5e-6, 2.4999999e-6],
    }];
    faces.extend((0..5).map(|_| plane([0.0, 0.0, 1.0])));
    let model = forged_box(faces, None);
    let entries = entries(&model, &id("bbbbbbbb")).unwrap();
    let face = named(entries, "Face1");
    assert_eq!(face.centroid, Some([0, 2, 2].map(Length::from_nm)));
    // An area of 4e-7 mm² is 0 to 6 places.
    assert_eq!(face.area, Some(Measure::from_millionths(0)));
}

#[test]
fn a_value_past_what_a_length_holds_refuses_the_read_naming_the_feature() {
    // A worker's centre of area at 1e13 mm, finite, so the description passes its checks,
    // but past what a Length (whole nanometres in an i64, 9.2e12 mm) holds: the read is
    // refused, naming the feature, never a panic or a wrapped value.
    for centroid in [[1e13, 0.0, 0.0], [0.0, -1e300, 0.0]] {
        let mut faces = vec![FaceDescription {
            surface: Surface::Other,
            area: 1.0,
            centroid,
        }];
        faces.extend((0..5).map(|_| plane([0.0, 0.0, 1.0])));
        let model = forged_box(faces, None);
        let refused = entries(&model, &id("bbbbbbbb")).unwrap_err();
        assert_eq!(refused.total(), 1);
        assert_eq!(refused.errors()[0].feature(), Some("bbbbbbbb"));
        let listed = list(&model, Revision::OPENED, None, None, 10).unwrap_err();
        assert_eq!(listed.errors()[0].feature(), Some("bbbbbbbb"));
    }
    // A radius whose diameter is past a length, though the radius isn't.
    let mut faces = vec![FaceDescription {
        surface: Surface::Cylinder {
            axis: Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
            radius: 5e12,
        },
        area: 1.0,
        centroid: [0.0; 3],
    }];
    faces.extend((0..5).map(|_| plane([0.0, 0.0, 1.0])));
    let model = forged_box(faces, None);
    let refused = entries(&model, &id("bbbbbbbb")).unwrap_err();
    assert_eq!(refused.errors()[0].feature(), Some("bbbbbbbb"));
}

// ---------------------------------------------------------------------------------------
// `elements`

#[test]
fn a_cursors_long_bad_name_is_refused_short_and_cut() {
    // A name that isn't one, a megabyte long, is said cut short, as the reader cuts every
    // string it echoes: the parameter says where it is in full.
    let model = model_of(TWO_BODIES);
    for name in [
        "x".repeat(1 << 20),
        format!("aaaaaaaa.box.Face1{}", " ".repeat(1 << 20)),
    ] {
        let cursor = Cursor {
            revision: Revision::OPENED,
            feature: id("aaaaaaaa"),
            name: ElementName::as_sent(&name),
        };
        let refused = list(&model, Revision::OPENED, None, Some(&cursor), 10).unwrap_err();
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.parameter(), Some("after"));
        assert!(
            error.message().len() < 600,
            "{} bytes",
            error.message().len()
        );
        assert!(error.message().contains("\"…"), "{}", error.message());
    }
    // A short one whole, its control character escaped.
    let cursor = Cursor {
        revision: Revision::OPENED,
        feature: id("aaaaaaaa"),
        name: ElementName::as_sent("Face\u{1b}"),
    };
    let refused = list(&model, Revision::OPENED, None, Some(&cursor), 10).unwrap_err();
    assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
    assert!(
        refused.errors()[0]
            .message()
            .starts_with(r#"the cursor's name, "Face\u{1b}", isn't an element's name: "#),
        "{}",
        refused.errors()[0].message()
    );
}

#[test]
fn a_cursor_naming_an_entry_the_listing_hasnt_is_refused_naming_after() {
    let model = model_of(TWO_BODIES);
    let whole = list(&model, Revision::OPENED, None, None, ENTRIES_AT_MOST).unwrap();
    for (feature, name, why) in [
        // Another feature's element, under the first body's ID.
        ("aaaaaaaa", "bbbbbbbb.box.Face1", "another feature's name"),
        // A name that's no name.
        ("aaaaaaaa", "", "an empty name"),
        (
            "aaaaaaaa",
            "aaaaaaaa.box.Face1 ",
            "a name with a space after it",
        ),
        ("aaaaaaaa", "aaaaaaaa.box.Face7", "a box's seventh face"),
        // A feature the document hasn't.
        (
            "zzzzzzzz",
            "zzzzzzzz.box.Face1",
            "a feature the document hasn't",
        ),
    ] {
        let cursor = Cursor {
            revision: Revision::OPENED,
            feature: id(feature),
            name: ElementName::as_sent(name),
        };
        let refused = list(&model, Revision::OPENED, None, Some(&cursor), 10).unwrap_err();
        assert_eq!(
            codes(&refused),
            [(ErrorCode::InvalidInput, Some("after".to_owned()))],
            "{why}"
        );
    }
    // A cursor of the second body, listing only the first: not this listing's.
    let second = Cursor {
        revision: Revision::OPENED,
        feature: id("bbbbbbbb"),
        name: ElementName::as_sent("bbbbbbbb.box.Face1"),
    };
    let refused = list(
        &model,
        Revision::OPENED,
        Some(&id("aaaaaaaa")),
        Some(&second),
        10,
    )
    .unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("after"));
    // The last entry's cursor: an empty page, every entry counted, nothing after it.
    let last = whole.entries.last().unwrap();
    let at_end = Cursor {
        revision: Revision::OPENED,
        feature: last.feature.clone(),
        name: ElementName::from(&*last.name),
    };
    let page = list(&model, Revision::OPENED, None, Some(&at_end), 10).unwrap();
    assert_eq!((page.entries.len(), page.total, page.next), (0, 52, None));
    // A cursor of the whole listing goes on in one feature's.
    let first = &whole.entries[0];
    let cursor = Cursor {
        revision: Revision::OPENED,
        feature: first.feature.clone(),
        name: ElementName::from(&*first.name),
    };
    let page = list(
        &model,
        Revision::OPENED,
        Some(&id("aaaaaaaa")),
        Some(&cursor),
        1000,
    )
    .unwrap();
    assert_eq!(page.entries, whole.entries[1..26]);
    // After every refusal, a listing still works.
    assert_eq!(
        list(&model, Revision::OPENED, None, None, 1).unwrap().total,
        52
    );
}

#[test]
fn a_page_of_one_walks_every_entry_once() {
    let model = model_of(TWO_BODIES);
    let whole = list(&model, Revision::OPENED, None, None, ENTRIES_AT_MOST).unwrap();
    let mut walked = Vec::new();
    let mut after: Option<Cursor> = None;
    loop {
        let page = list(&model, Revision::OPENED, None, after.as_ref(), 1).unwrap();
        assert!(page.entries.len() <= 1);
        walked.extend(page.entries);
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(walked, whole.entries);
}

#[test]
fn a_model_with_no_bodies_lists_and_queries_nothing() {
    // A sketch alone makes no solid: no bodies, so nothing listed, none matched.
    let model = model_of(SKETCH_ONLY);
    assert!(model.bodies().is_empty());
    let listed = list(&model, Revision::OPENED, None, None, 10).unwrap();
    assert_eq!(
        (listed.entries.len(), listed.total, listed.next),
        (0, 0, None)
    );
    let mut largest = Query::of(ElementKind::Face, 1);
    largest.order = Some(Order {
        by: MeasureName::Area,
        descending: true,
    });
    let got = query(&model, &largest).unwrap();
    assert_eq!((got.entries.len(), got.matched, got.tied), (0, 0, None));
    // The sketch named is refused, naming feature.
    let refused = list(&model, Revision::OPENED, Some(&id("k7f2qx3m")), None, 10).unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("feature"));
}

#[test]
fn a_bad_limit_and_a_feature_that_makes_no_solid_are_both_refused() {
    // `elements` and `query` give every reason a call is refused: a limit of 0 and a
    // feature that's a sketch are two, the feature's first in the request's order. Broke
    // before break-it's fix: the limit alone, the feature looked at only once it passed.
    let model = model_of(SKETCH_ONLY);
    let refused = list(&model, Revision::OPENED, Some(&id("k7f2qx3m")), None, 0).unwrap_err();
    let mut params: Vec<Option<String>> = codes(&refused).into_iter().map(|(_, p)| p).collect();
    params.sort();
    assert_eq!(
        params,
        [Some("feature".to_owned()), Some("limit".to_owned())],
        "{refused:?}"
    );
    let mut q = Query::of(ElementKind::Face, 0);
    q.feature = Some(id("k7f2qx3m"));
    let refused = query(&model, &q).unwrap_err();
    assert_eq!(
        codes(&refused),
        [
            (ErrorCode::InvalidInput, Some("feature".to_owned())),
            (ErrorCode::InvalidInput, Some("limit".to_owned())),
        ],
        "{refused:?}"
    );
}

#[test]
fn a_solid_not_described_is_refused_naming_it_and_its_neighbour_still_lists() {
    // The second box's description refused by the worker: listing it, or every body, is
    // refused, naming it and why; the first box still lists, and a query of it still runs.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| match op {
        Op::Describe { shape, .. } | Op::DescribeAll { shape }
            if FakeKernel::describe(shape).is_some_and(|d| d.contains("5, 0, 0")) =>
        {
            Some(silico_core::Error::new(
                ErrorCode::KernelFailed,
                "no describing today",
            ))
        }
        _ => None,
    });
    let model = published(&engine, checked(TWO_BODIES));
    let refused = entries(&model, &id("bbbbbbbb")).unwrap_err();
    assert_eq!(refused.errors()[0].feature(), Some("bbbbbbbb"));
    assert_eq!(refused.errors()[0].parameter(), Some("feature"));
    assert_eq!(refused.errors()[0].message(), "no describing today");
    // Of every body, the request's feature null: the body named as the error's feature,
    // and no parameter, since the request named none.
    let all = list(&model, Revision::OPENED, None, None, 10).unwrap_err();
    assert_eq!(all.errors()[0].feature(), Some("bbbbbbbb"));
    assert_eq!(all.errors()[0].parameter(), None);
    assert_eq!(all.total(), 1);
    assert_eq!(
        list(&model, Revision::OPENED, Some(&id("aaaaaaaa")), None, 100)
            .unwrap()
            .total,
        26
    );
    let mut faces = Query::of(ElementKind::Face, 10);
    faces.feature = Some(id("aaaaaaaa"));
    assert_eq!(query(&model, &faces).unwrap().matched, 6);
}

#[test]
fn every_body_not_described_is_said_not_only_the_first() {
    // Both boxes' descriptions refused: a listing of every body says each, in timeline
    // order, two in all.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        matches!(op, Op::Describe { .. } | Op::DescribeAll { .. })
            .then(|| silico_core::Error::new(ErrorCode::KernelFailed, "no describing today"))
    });
    let model = published(&engine, checked(TWO_BODIES));
    let all = list(&model, Revision::OPENED, None, None, 10).unwrap_err();
    assert_eq!(all.total(), 2);
    let features: Vec<Option<&str>> = all.errors().iter().map(|e| e.feature()).collect();
    assert_eq!(features, [Some("aaaaaaaa"), Some("bbbbbbbb")]);
}

#[test]
fn a_listings_refusals_come_in_its_requests_order_feature_then_cursor_then_limit() {
    // A feature the document hasn't, a cursor of another revision, and a limit of 0:
    // refused in that order, the order the request gives its fields.
    let model = model_of(TWO_BODIES);
    let cursor = Cursor {
        revision: Revision::OPENED.next().unwrap(),
        feature: id("bbbbbbbb"),
        name: ElementName::as_sent("bbbbbbbb.box.Face1"),
    };
    let refused = list(
        &model,
        Revision::OPENED,
        Some(&id("zzzzzzzz")),
        Some(&cursor),
        0,
    )
    .unwrap_err();
    let parameters: Vec<Option<&str>> = refused.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(parameters, [Some("feature"), Some("after"), Some("limit")]);
}

// ---------------------------------------------------------------------------------------
// `query`: facing

/// A box whose six faces are planes facing +z, -z, +x, (1, 1, -0)/√2, -y, and (1, 0, 1)/√2.
fn facing_box() -> Arc<Model> {
    let h = FRAC_1_SQRT_2;
    forged_box(
        [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [h, h, -0.0],
            [-0.0, -1.0, -0.0],
            [h, 0.0, h],
        ]
        .into_iter()
        .map(plane)
        .collect(),
        None,
    )
}

/// The faces' numbers (`FaceN`) facing `direction` within `within`.
fn facing(model: &Model, direction: [f64; 3], within: Angle) -> Vec<u32> {
    let mut q = Query::of(ElementKind::Face, 1000);
    q.facing = Some(Facing { direction, within });
    let got = query(model, &q).unwrap_or_else(|e| panic!("{direction:?}: {e:?}"));
    got.entries
        .iter()
        .map(|e| {
            e.name
                .to_string()
                .trim_start_matches("bbbbbbbb.box.Face")
                .parse()
                .unwrap()
        })
        .collect()
}

#[test]
fn facing_within_0_90_and_180_degrees_of_axes_keeps_what_it_should() {
    let model = facing_box();
    let up = [0.0, 0.0, 1.0];
    let ndeg = Angle::from_ndeg;
    assert_eq!(facing(&model, up, ndeg(0)), [1]);
    assert_eq!(facing(&model, [0.0, 0.0, -1.0], ndeg(0)), [2]);
    // 45° exactly, as written: (1, 0, 1)/√2 against +z.
    assert_eq!(facing(&model, up, deg(45)), [1, 6]);
    assert_eq!(facing(&model, up, ndeg(44_999_999_999)), [1]);
    // Orthogonal normals are 90° off, -0 or not.
    assert_eq!(facing(&model, up, deg(90)), [1, 3, 4, 5, 6]);
    assert_eq!(facing(&model, up, ndeg(89_999_999_999)), [1, 6]);
    // The opposite normal is 180° off: within 180 everything faces.
    assert_eq!(facing(&model, up, deg(180)), [1, 2, 3, 4, 5, 6]);
    assert_eq!(facing(&model, up, ndeg(179_999_999_999)), [1, 3, 4, 5, 6]);
    // Against +x: (1, 1, 0)/√2 and (1, 0, 1)/√2 are 45° off.
    assert_eq!(facing(&model, [1.0, 0.0, 0.0], deg(45)), [3, 4, 6]);
    assert_eq!(facing(&model, [1.0, 0.0, 0.0], ndeg(44_999_999_999)), [3]);
}

#[test]
fn facing_a_direction_of_any_size_is_facing_its_unit_direction() {
    // A direction's size doesn't change which way it points: a direction 1e-160 or 1e-200
    // long, or 1e200 long, faces as its unit direction does. Broke before break-it's fix:
    // its squares underflowed or overflowed, so (1e-160, 0, 0) put (1, 1, 0)/√2 under 45°
    // off it, and (0, 0, 1e-200) or (0, 0, 1e200) was refused as "not a direction".
    let model = facing_box();
    let want = |d: [f64; 3], within: Angle| facing(&model, d, within);
    for scale in [2.0, 1e-100, 1e100] {
        assert_eq!(want([0.0, 0.0, scale], deg(45)), [1, 6], "scale {scale}");
    }
    for scale in [1e-160, 1e-200, 1e-310, 1e160, 1e200, f64::MAX] {
        assert_eq!(
            want([0.0, 0.0, scale], deg(45)),
            [1, 6],
            "(0, 0, {scale}) within 45"
        );
        assert_eq!(
            want([scale, 0.0, 0.0], deg(45)),
            [3, 4, 6],
            "({scale}, 0, 0) within 45"
        );
        assert_eq!(
            want([scale, 0.0, 0.0], Angle::from_ndeg(44_999_999_999)),
            [3],
            "({scale}, 0, 0) within 44.999999999"
        );
    }
}

#[test]
fn a_direction_that_isnt_one_is_refused_naming_facing_direction() {
    let model = facing_box();
    for d in [
        [0.0, 0.0, 0.0],
        [-0.0, -0.0, -0.0],
        [f64::NAN, 0.0, 1.0],
        [0.0, f64::INFINITY, 0.0],
        [f64::NEG_INFINITY, 0.0, 0.0],
        [f64::NAN, f64::NAN, f64::NAN],
    ] {
        let mut q = Query::of(ElementKind::Face, 10);
        q.facing = Some(Facing {
            direction: d,
            within: deg(1),
        });
        let refused = query(&model, &q).unwrap_err();
        assert_eq!(
            codes(&refused),
            [(ErrorCode::InvalidInput, Some("facing.direction".to_owned()))],
            "{d:?}"
        );
    }
    // A negative within, and one past 180, are refused naming facing.within.
    for within in [Angle::from_ndeg(-1), Angle::from_ndeg(180_000_000_001)] {
        let mut q = Query::of(ElementKind::Face, 10);
        q.facing = Some(Facing {
            direction: [0.0, 0.0, 1.0],
            within,
        });
        let refused = query(&model, &q).unwrap_err();
        assert_eq!(refused.errors()[0].parameter(), Some("facing.within"));
    }
}

#[test]
fn on_the_wire_a_zero_direction_is_the_readers_and_hides_the_commands_own_refusals_until_the_shape_is_right()
 {
    // docs/design/m4.4-describe.md, *On the wire*: a zero direction is among the reader's
    // refusals of a request's shape, beside every other (a kind of solid, in the command's
    // words); a limit past 1,000 is the command's; and "a request refused for its shape
    // isn't run, so the command's own reasons come once the shape is right". So a request
    // wrong in all three is refused for the kind and the direction, in the shape's order,
    // and not for the limit; with the direction mended, for the kind alone; with the kind
    // mended too, the command runs and refuses the limit, in the words of M4.1's example
    // (docs/design/m4.1-command-api.md, `elements`' refusal of 5000). The Rust `Query`, which
    // has no reader, gives the zero direction beside the limit at once: tests/entries.rs's
    // a_query_that_cant_mean_anything_is_refused_with_every_reason.
    use silico_core::command::wire::elements::QueryRequest;
    let request = |kind: &str, direction: &str| {
        format!(
            r#"{{"session": "s1", "model": "current", "feature": null, "kind": "{kind}", "type": null, "stable": null, "facing": {{"direction": {direction}, "within": 1}}, "measures": [], "order": null, "limit": 5000}}"#
        )
    };
    let read = |text: &str| {
        QueryRequest::read(&silico_core::document::json::read(text.as_bytes()).unwrap())
    };
    let said = |refused: &silico_core::command::Problems| -> Vec<(String, String)> {
        refused
            .errors()
            .iter()
            .map(|e| {
                (
                    e.parameter().unwrap_or("").to_owned(),
                    e.message().to_owned(),
                )
            })
            .collect()
    };
    let refused = read(&request("solid", "[0, 0, 0]")).unwrap_err();
    assert_eq!(
        said(&refused),
        [
            (
                "kind".to_owned(),
                "kind must be face, edge, or vertex, got solid".to_owned()
            ),
            (
                "facing.direction".to_owned(),
                "facing.direction must be a direction, got [0, 0, 0]".to_owned()
            ),
        ]
    );
    assert_eq!(refused.total(), 2);
    assert!(
        refused
            .errors()
            .iter()
            .all(|e| e.code() == ErrorCode::InvalidInput)
    );
    // The direction mended: the kind is still the shape's, so the limit is still unsaid.
    let refused = read(&request("solid", "[0, 0, 1]")).unwrap_err();
    assert_eq!(
        codes(&refused),
        [(ErrorCode::InvalidInput, Some("kind".to_owned()))]
    );
    // The shape right: the command runs, and refuses the limit.
    let request = read(&request("face", "[0, 0, 1]")).unwrap();
    let refused = query(&model_of(BOX), &request.query).unwrap_err();
    assert_eq!(
        said(&refused),
        [(
            "limit".to_owned(),
            "limit must be from 1 to 1000, got 5000".to_owned()
        )]
    );
    assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
}

// ---------------------------------------------------------------------------------------
// `query`: ties at 1,000 and 1,001

/// 170 boxes fused: one body of 1,020 faces. Its face k (by the kernel's number) is a
/// cylinder of radius k mm, so a radius range picks faces by number; its area 1 for k up
/// to 17, 10 for 18 and 19, and 5 from 20 to 1,020 (1,001 faces).
fn tie_model() -> Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let face = |k: u32| FaceDescription {
        surface: Surface::Cylinder {
            axis: Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
            radius: f64::from(k),
        },
        area: match k {
            ..=17 => 1.0,
            18 | 19 => 10.0,
            _ => 5.0,
        },
        centroid: [0.0; 3],
    };
    engine.kernel().answer_instead(move |op| match op {
        Op::Describe {
            shape,
            kind: ElementKind::Face,
            first,
            count,
        } if FakeKernel::counts(shape).is_some_and(|c| c.faces == 1020) => Some(Reply::Described {
            total: 1020,
            description: Description {
                faces: (*first..*first + *count).map(face).collect(),
                edges: Vec::new(),
                vertices: Vec::new(),
            },
        }),
        _ => None,
    });
    published(&engine, checked(&super::describing::fused_boxes(170)))
}

fn mm(n: i64) -> Option<Quantity> {
    Some(Quantity::Length(Length::from_nm(n * 1_000_000)))
}

/// Faces of radius `lo` to `hi` mm and, if given, area at most `area`, by area, `limit`.
fn by_area(lo: i64, hi: i64, area: Option<i128>, descending: bool, limit: u32) -> Query {
    let mut q = Query::of(ElementKind::Face, limit);
    q.measures = vec![Range {
        measure: MeasureName::Radius,
        at_least: mm(lo),
        at_most: mm(hi),
    }];
    if let Some(area) = area {
        q.measures.push(Range {
            measure: MeasureName::Area,
            at_least: None,
            at_most: Some(Quantity::Area(Measure::from_millionths(area * 1_000_000))),
        });
    }
    q.order = Some(Order {
        by: MeasureName::Area,
        descending,
    });
    q
}

#[test]
fn a_tie_ending_at_1000_is_returned_and_one_ending_at_1001_isnt() {
    let model = tie_model();
    let area = |e: &Entry| e.area.unwrap().millionths() / 1_000_000;
    let run = |q: &Query| {
        let got = query(&model, q).unwrap();
        (
            got.entries.len(),
            got.matched,
            got.tied.as_ref().map(|t| (t.count, t.included)),
            got,
        )
    };
    // Radius 1 to 1,017, largest first, 3: two of 10, then a tie of 998 at the cut,
    // ending at the 1,000th: all of it returned.
    let (n, matched, tied, got) = run(&by_area(1, 1017, None, true, 3));
    assert_eq!((n, matched, tied), (1000, 1017, Some((998, true))));
    assert_eq!(
        got.entries[..2].iter().map(area).collect::<Vec<_>>(),
        [10, 10]
    );
    assert!(got.entries[2..].iter().all(|e| area(e) == 5));
    // To 1,018: a tie of 999 ending at the 1,001st: none of it.
    let (n, matched, tied, got) = run(&by_area(1, 1018, None, true, 3));
    assert_eq!((n, matched, tied), (2, 1018, Some((999, false))));
    assert!(got.entries.iter().all(|e| area(e) == 10));
    // Area at most 5, to 1,019, 1,000: 1,000 of 5 then 17 of 1, cut between them: no tie.
    let (n, matched, tied, _) = run(&by_area(1, 1019, Some(5), true, 1000));
    assert_eq!((n, matched, tied), (1000, 1017, None));
    // The same, 999: a tie of 1,000 at the cut, ending at the 1,000th: returned.
    let (n, _, tied, _) = run(&by_area(1, 1019, Some(5), true, 999));
    assert_eq!((n, tied), (1000, Some((1000, true))));
    // To 1,020, 1,000: a tie of 1,001 at the cut: none of it.
    let (n, matched, tied, _) = run(&by_area(1, 1020, Some(5), true, 1000));
    assert_eq!((n, matched, tied), (0, 1018, Some((1001, false))));
    // Smallest first, to 1,017, 18: 17 of 1, then a tie of 998 from the 18th, ending at
    // the 1,015th: none of it.
    let (n, _, tied, got) = run(&by_area(1, 1017, None, false, 18));
    assert_eq!((n, tied), (17, Some((998, false))));
    assert!(got.entries.iter().all(|e| area(e) == 1));
    // A cut between two values is no tie: 17 smallest, or the 2 largest.
    let (n, _, tied, _) = run(&by_area(1, 1017, None, false, 17));
    assert_eq!((n, tied), (17, None));
    let (n, _, tied, _) = run(&by_area(1, 1017, None, true, 2));
    assert_eq!((n, tied), (2, None));
    // A limit past every match cuts nothing.
    let (n, _, tied, _) = run(&by_area(1, 19, None, true, 1000));
    assert_eq!((n, tied), (19, None));
}

#[test]
fn an_order_by_a_measure_keeps_only_entries_that_have_it_and_descending_reverses_it() {
    // Six cones of half-angles -30, -0.5, 0, 0.5 (twice), and 60 degrees: by half-angle
    // descending, 60, the 0.5 tie (the cut's, limit 2, returned whole), ...; by radius
    // (which a cone hasn't), nothing.
    let up = [0.0, 0.0, 1.0];
    let r = |d: f64| d.to_radians();
    let model = forged_box(
        [-30.0, -0.5, 0.0, 0.5, 0.5, 60.0]
            .into_iter()
            .map(|a| cone([0.0; 3], up, 1.0, r(a)))
            .collect(),
        None,
    );
    let mut q = Query::of(ElementKind::Face, 2);
    q.order = Some(Order {
        by: MeasureName::HalfAngle,
        descending: true,
    });
    let got = query(&model, &q).unwrap();
    let angles: Vec<i64> = got
        .entries
        .iter()
        .map(|e| e.half_angle.unwrap().ndeg())
        .collect();
    assert_eq!(angles, [60_000_000_000, 500_000_000, 500_000_000]);
    assert_eq!(got.tied.map(|t| (t.count, t.included)), Some((2, true)));
    q.order = Some(Order {
        by: MeasureName::HalfAngle,
        descending: false,
    });
    q.limit = 1000;
    let got = query(&model, &q).unwrap();
    let angles: Vec<i64> = got
        .entries
        .iter()
        .map(|e| e.half_angle.unwrap().ndeg())
        .collect();
    assert_eq!(
        angles,
        [
            -30_000_000_000,
            -500_000_000,
            0,
            500_000_000,
            500_000_000,
            60_000_000_000
        ]
    );
    // Half-angles of -0.5 to 0.5, both ends included.
    q.measures = vec![Range {
        measure: MeasureName::HalfAngle,
        at_least: Some(Quantity::Angle(Angle::from_ndeg(-500_000_000))),
        at_most: Some(Quantity::Angle(Angle::from_ndeg(500_000_000))),
    }];
    assert_eq!(query(&model, &q).unwrap().matched, 4);
    q.measures.clear();
    q.order = Some(Order {
        by: MeasureName::Radius,
        descending: true,
    });
    let got = query(&model, &q).unwrap();
    assert_eq!((got.matched, got.entries.len(), got.tied), (0, 0, None));
}

// ---------------------------------------------------------------------------------------
// `query`: stability and refusals

#[test]
fn more_ranges_than_there_are_measures_are_refused_as_one_reason_naming_measures() {
    // The reliability review's: a million ranges built a million errors and checked every
    // range against every element. Past one a measure (7), the list is refused naming
    // `measures`, none of its ranges looked at: 150 ranges of a face's length (which a face
    // hasn't), and a limit of 0, are two reasons, not 151.
    assert_eq!(MEASURES_AT_MOST, 7);
    let model = model_of(BOX);
    let length = Range {
        measure: MeasureName::Length,
        at_least: None,
        at_most: None,
    };
    let mut q = Query::of(ElementKind::Face, 0);
    q.measures = vec![length.clone(); 150];
    let refused = query(&model, &q).unwrap_err();
    assert_eq!(
        codes(&refused),
        [
            (ErrorCode::InvalidInput, Some("measures".to_owned())),
            (ErrorCode::InvalidInput, Some("limit".to_owned())),
        ]
    );
    assert_eq!(
        refused.errors()[0].message(),
        "measures holds at most 7 ranges, as many as there are measures, got 150"
    );
    // At the limit, each range is checked: seven of a face's length, seven reasons.
    q.measures = vec![length; 7];
    q.limit = 1;
    let refused = query(&model, &q).unwrap_err();
    assert_eq!(refused.total(), 7);
    assert_eq!(refused.errors()[6].parameter(), Some("measures.6.measure"));
    // Eight good ranges are refused too; seven run.
    let area = Range {
        measure: MeasureName::Area,
        at_least: Some(Quantity::Area(Measure::from_millionths(0))),
        at_most: None,
    };
    q.measures = vec![area.clone(); 8];
    assert_eq!(
        codes(&query(&model, &q).unwrap_err()),
        [(ErrorCode::InvalidInput, Some("measures".to_owned()))]
    );
    q.measures = vec![area; 7];
    q.limit = 10;
    assert_eq!(query(&model, &q).unwrap().matched, 6);
}

#[test]
fn a_solid_kind_is_refused_with_the_other_reasons_that_dont_depend_on_the_kind() {
    // Kind solid, a zero direction, a within past 180, an empty range, and a limit of 0:
    // the kind, and every other reason a kind doesn't change, given at once. Broke before
    // break-it's fix: the kind alone.
    let model = model_of(BOX);
    let mut q = Query::of(ElementKind::Solid, 0);
    q.facing = Some(Facing {
        direction: [0.0, 0.0, 0.0],
        within: deg(181),
    });
    q.measures = vec![Range {
        measure: MeasureName::Area,
        at_least: Some(Quantity::Area(Measure::from_millionths(2))),
        at_most: Some(Quantity::Area(Measure::from_millionths(1))),
    }];
    let refused = query(&model, &q).unwrap_err();
    let params: Vec<Option<String>> = codes(&refused).into_iter().map(|(_, p)| p).collect();
    assert_eq!(
        params,
        [
            "kind",
            "facing.direction",
            "facing.within",
            "measures.0.at_least",
            "limit"
        ]
        .map(|p| Some(p.to_owned())),
        "{refused:?}"
    );
}

#[test]
fn a_query_of_a_feature_that_failed_gives_its_failure() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "MakeCylinder")
            .then(|| silico_core::Error::new(ErrorCode::InvalidInput, "no pins today"))
    });
    let model = published(&engine, checked(EXAMPLE));
    let mut q = Query::of(ElementKind::Face, 10);
    q.feature = Some(id("a91cw0de"));
    let refused = query(&model, &q).unwrap_err();
    assert_eq!(refused.errors()[0].message(), "no pins today");
    q.feature = Some(id("m3tq8v0z"));
    let blocked = query(&model, &q).unwrap_err();
    assert_eq!(blocked.errors()[0].parameter(), Some("feature"));
    // With no feature, the bodies: the block alone, its cut blocked.
    q.feature = None;
    assert_eq!(query(&model, &q).unwrap().matched, 6);
}

#[test]
fn a_type_keeps_only_its_own_and_facing_needs_a_plane() {
    // The skeptic's: a cylinder among five planes. `type: cylinder` keeps it alone, `type:
    // plane` the five; `facing` under `type: cylinder` is refused, naming facing, and under
    // `type: plane` it runs.
    let mut faces = vec![FaceDescription {
        surface: Surface::Cylinder {
            axis: Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
            radius: 2.5,
        },
        area: 1.0,
        centroid: [0.0; 3],
    }];
    faces.extend((0..5).map(|_| plane([0.0, 0.0, 1.0])));
    let model = forged_box(faces, None);
    let mut q = Query::of(ElementKind::Face, 100);
    q.r#type = Some(ElementType::Cylinder);
    let got = query(&model, &q).unwrap();
    let names: Vec<String> = got.entries.iter().map(|e| e.name.to_string()).collect();
    assert_eq!(
        (got.matched, names),
        (1, vec!["bbbbbbbb.box.Face1".to_owned()])
    );
    q.r#type = Some(ElementType::Plane);
    let planes = query(&model, &q).unwrap();
    assert_eq!(planes.matched, 5);
    assert!(
        planes
            .entries
            .iter()
            .all(|e| e.r#type == ElementType::Plane)
    );
    // A type no face here is: none, and no refusal.
    q.r#type = Some(ElementType::Torus);
    assert_eq!(query(&model, &q).unwrap().matched, 0);
    q.r#type = Some(ElementType::Cylinder);
    q.facing = Some(Facing {
        direction: [0.0, 0.0, 1.0],
        within: deg(10),
    });
    let refused = query(&model, &q).unwrap_err();
    assert_eq!(
        codes(&refused),
        [(ErrorCode::InvalidInput, Some("facing".to_owned()))]
    );
    assert_eq!(
        refused.errors()[0].message(),
        "only planar faces face a direction, and type isn't plane"
    );
    q.r#type = Some(ElementType::Plane);
    assert_eq!(query(&model, &q).unwrap().matched, 5);
}

#[test]
fn a_range_of_a_measure_an_entry_hasnt_leaves_it_out() {
    // The skeptic's: of a cylinder (radius 2.5) and five planes, a radius range, however
    // wide or open, keeps the cylinder alone: a plane has no radius, so no range of one
    // holds it.
    let mut faces = vec![FaceDescription {
        surface: Surface::Cylinder {
            axis: Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
            radius: 2.5,
        },
        area: 1.0,
        centroid: [0.0; 3],
    }];
    faces.extend((0..5).map(|_| plane([0.0, 0.0, 1.0])));
    let model = forged_box(faces, None);
    for (at_least, at_most, matched) in [
        (None, None, 1),
        (mm(0), None, 1),
        (mm(2), mm(3), 1),
        (mm(3), None, 0),
    ] {
        let mut q = Query::of(ElementKind::Face, 100);
        q.measures = vec![Range {
            measure: MeasureName::Radius,
            at_least,
            at_most,
        }];
        let got = query(&model, &q).unwrap();
        assert_eq!(got.matched, matched, "{at_least:?} to {at_most:?}");
        assert!(
            got.entries
                .iter()
                .all(|e| e.r#type == ElementType::Cylinder)
        );
    }
}

#[test]
fn an_edges_start_and_end_are_its_curves_own_in_order() {
    // The skeptic's: each forged line n runs from (n, 0, 0) to (n, 1, 0); its entry's start
    // and end are those, in that order, never swapped.
    let model = forged_box(
        (0..6).map(|_| plane([0.0, 0.0, 1.0])).collect(),
        Some((1..=12).map(line).collect()),
    );
    let entries = entries(&model, &id("bbbbbbbb")).unwrap();
    for n in 1..=12 {
        let edge = named(entries, &format!("Edge{n}"));
        let x = i64::from(n) * 1_000_000;
        assert_eq!(edge.start, Some([x, 0, 0].map(Length::from_nm)), "Edge{n}");
        assert_eq!(
            edge.end,
            Some([x, 1_000_000, 0].map(Length::from_nm)),
            "Edge{n}"
        );
        assert_eq!(
            edge.midpoint,
            Some([x, 500_000, 0].map(Length::from_nm)),
            "Edge{n}"
        );
    }
}

/// Two boxes, and their fusion `rrrrrrrr`.
const FUSED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "First", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Second", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "rrrrrrrr", "type": "fuse", "v": 1, "name": "Both", "base": "aaaaaaaa", "tool": "bbbbbbbb"}]}"#;

#[test]
fn an_element_nothing_names_is_unstable_and_stable_filters_on_it() {
    // The skeptic's: a fake whose fuse gives the base box back, with a history that reports
    // nothing of the base's Face1 or Edge1 (Edge1 bounding Face1 alone, and no vertex) and
    // nothing of the tool. Face1's outer wire has an edge nothing names, so it's named by
    // its number, `rrrrrrrr.fuse.Face1`, which is unstable (N3, as naming.rs's
    // a_face_is_named_from_below_only_when_every_edge_of_its_outer_wire_is_named): its
    // entry isn't stable, `stable: false` keeps it alone, and `stable: true` the rest.
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    let none = |n: usize| vec![ElementFate::default(); n];
    let face1 = |rest: Vec<Vec<u32>>| -> Vec<Vec<u32>> {
        std::iter::once(vec![1, 2]).chain(rest).collect()
    };
    let history = History {
        inputs: vec![
            InputHistory {
                faces: none(1).into_iter().chain((2..=6).map(kept)).collect(),
                edges: none(1).into_iter().chain((2..=12).map(kept)).collect(),
                vertices: (1..=8).map(kept).collect(),
            },
            InputHistory {
                faces: none(6),
                edges: none(12),
                vertices: none(8),
            },
        ],
        result: ElementCounts {
            faces: 6,
            edges: 12,
            vertices: 8,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: face1(vec![vec![]; 5]),
            outer_edges: face1(vec![vec![]; 5]),
            edge_vertices: vec![vec![]; 12],
        },
    };
    history.validate(2).unwrap();
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().answer_instead(move |op| match op {
        Op::Boolean {
            op: BooleanOp::Fuse,
            base,
            ..
        } => Some(Reply::Built(BuiltData {
            shape: base.clone(),
            history: history.clone(),
        })),
        _ => None,
    });
    let model = published(&engine, checked(FUSED));
    let fused = id("rrrrrrrr");
    let all = entries(&model, &fused).unwrap();
    let by_name = |name: &str| {
        all.iter()
            .find(|e| e.name.to_string() == name)
            .unwrap_or_else(|| {
                panic!(
                    "no {name}: {:?}",
                    all.iter().map(|e| e.name.to_string()).collect::<Vec<_>>()
                )
            })
    };
    let face = by_name("rrrrrrrr.fuse.Face1");
    assert!(!face.stable);
    assert!(face.aliases.is_empty());
    // The faces kept are their box's, stable.
    for n in 2..=6 {
        assert!(by_name(&format!("aaaaaaaa.box.Face{n}")).stable, "Face{n}");
    }
    let mut q = Query::of(ElementKind::Face, 100);
    q.feature = Some(fused.clone());
    q.stable = Some(false);
    let unstable = query(&model, &q).unwrap();
    assert_eq!(unstable.entries, std::slice::from_ref(face));
    q.stable = Some(true);
    let stable = query(&model, &q).unwrap();
    assert_eq!(stable.matched, 5);
    assert!(
        stable
            .entries
            .iter()
            .all(|e| e.stable && e.name != face.name)
    );
    q.stable = None;
    assert_eq!(query(&model, &q).unwrap().matched, 6);
    // Of its edges, those `stable: false` keeps are exactly those whose entry isn't stable,
    // the edge nothing named among them.
    let mut edges = Query::of(ElementKind::Edge, 100);
    edges.feature = Some(fused);
    edges.stable = Some(false);
    let unstable = query(&model, &edges).unwrap().entries;
    let want: Vec<Entry> = all
        .iter()
        .filter(|e| e.kind == ElementKind::Edge && !e.stable)
        .cloned()
        .collect();
    assert_eq!(unstable, want);
    assert!(!unstable.is_empty());
}

// ---------------------------------------------------------------------------------------
// Sessions

#[test]
fn a_session_not_open_is_refused_naming_session_for_either_model() {
    let f = fixture();
    let s = f.open("box.silico", BOX);
    // Read once, so a model is published, then closed.
    let current = answer(
        f.sessions
            .elements(&s, ModelChoice::Current, None, None, 10),
    );
    assert_eq!(current.result.unwrap().total, 26);
    answer(f.sessions.close(&s));
    for model in [ModelChoice::Current, ModelChoice::Published] {
        let listed = answer(f.sessions.elements(&s, model, None, None, 10));
        let queried = answer(f.sessions.query(&s, model, Query::of(ElementKind::Face, 1)));
        for problems in [listed.result.unwrap_err(), queried.result.unwrap_err()] {
            assert_eq!(
                codes(&problems),
                [(ErrorCode::InvalidInput, Some("session".to_owned()))],
                "{model:?}"
            );
        }
    }
}

/// The sketch test's rectangle (tests/sketch.rs), alone.
const SKETCH_ONLY: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {
      "id": "k7f2qx3m",
      "type": "sketch",
      "v": 1,
      "name": "Base",
      "plane": "xy",
      "points": [
        {"id": "0a1b2c3d", "x": 0, "y": 0, "construction": false},
        {"id": "2p3q4r5s", "x": 0, "y": 20, "construction": false},
        {"id": "4e5f6g7h", "x": 40, "y": 0, "construction": false},
        {"id": "8j9k0m1n", "x": 40, "y": 20, "construction": false}
      ],
      "lines": [
        {"id": "6t7v8w9x", "from": "0a1b2c3d", "to": "4e5f6g7h", "construction": false},
        {"id": "a0b1c2d3", "from": "4e5f6g7h", "to": "8j9k0m1n", "construction": false},
        {"id": "j8k9m0n1", "from": "8j9k0m1n", "to": "2p3q4r5s", "construction": false},
        {"id": "p2q3r4s5", "from": "2p3q4r5s", "to": "0a1b2c3d", "construction": false}
      ],
      "circles": [],
      "arcs": [],
      "constraints": []
    }
  ]
}
"#;
