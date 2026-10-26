//! break-it: `distance`'s `center_to_element` (`silico_core::command::measure::distance`) on
//! the fake kernel, the boxes' descriptions forged so their elements have the centres and
//! carriers each rule needs: a cylinder's axis a fraction of a nanodegree either side of
//! parallel to a plane and a line, turned end for end, lying in the plane, through the line;
//! a point centre asked of the other feature's shape, either way round; a worker's reply to
//! the centre's distance that no pair has (negative, NaN, no pair, the wrong reply, past
//! what a length holds), each a structured error and the kernel fine after; a centre past
//! the point's bound; and the centre's request held while its session is cancelled or
//! closed. The fake's point distance is the point's from (n, 0, 0), the element's number n,
//! so each value is by hand.

use std::sync::Arc;
use std::time::Duration;

use super::listing::{answer, fixture};
use super::model::{checked, published};

use silico_core::ErrorCode;
use silico_core::command::ModelChoice;
use silico_core::command::measure::distance;
use silico_core::document::Length;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    Axis, Description, EdgeCurve, EdgeDescription, ElementKind, FaceDescription, Op, Surface,
};
use silico_core::recompute::{Model, Recompute};
use silico_protocol::Reply;

const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}]}"#;

const TWO: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "First", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Second", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}]}"#;

fn mm(whole: i64) -> Length {
    Length::from_nm(whole * 1_000_000)
}

fn pair(a: &str, b: &str) -> [String; 2] {
    [a.to_owned(), b.to_owned()]
}

fn name(element: &str) -> String {
    format!("bbbbbbbb.box.{element}")
}

fn cylinder(origin: [f64; 3], direction: [f64; 3]) -> FaceDescription {
    FaceDescription {
        surface: Surface::Cylinder {
            axis: Axis { origin, direction },
            radius: 1.0,
        },
        area: 1.0,
        centroid: origin,
    }
}

fn circle(centre: [f64; 3]) -> EdgeDescription {
    let on = [centre[0] + 1.0, centre[1], centre[2]];
    EdgeDescription {
        curve: EdgeCurve::Circle {
            axis: Axis {
                origin: centre,
                direction: [0.0, 0.0, 1.0],
            },
            radius: 1.0,
        },
        length: 1.0,
        start: on,
        end: on,
        middle: on,
    }
}

/// A direction `degrees` off x toward z, in the xz plane.
fn off_x(degrees: f64) -> [f64; 3] {
    let r = degrees.to_radians();
    [libm::cos(r), 0.0, libm::sin(r)]
}

/// The forged box: Face1 the plane z = 0 (centred (1, 0, 0)); Face2–Face5 cylinders whose
/// axes are given; Face6 of type other. Edge1 the line along x from the origin to (10, 0,
/// 0); Edge2 a circle centred `far`; the rest of type other. Vertices at (n, 0, 0) but
/// Vertex1, at (4, 4, 0).
fn description(axes: [([f64; 3], [f64; 3]); 4], far: [f64; 3]) -> Description {
    let faces = (1..=6u32)
        .map(|n| match n {
            1 => FaceDescription {
                surface: Surface::Plane {
                    normal: [0.0, 0.0, 1.0],
                },
                area: 1.0,
                centroid: [1.0, 0.0, 0.0],
            },
            2..=5 => {
                let (origin, direction) = axes[(n - 2) as usize];
                cylinder(origin, direction)
            }
            n => FaceDescription {
                surface: Surface::Other,
                area: 1.0,
                centroid: [f64::from(n), 0.0, 0.0],
            },
        })
        .collect();
    let edges = (1..=12u32)
        .map(|n| match n {
            1 => EdgeDescription {
                curve: EdgeCurve::Line,
                length: 10.0,
                start: [0.0; 3],
                end: [10.0, 0.0, 0.0],
                middle: [5.0, 0.0, 0.0],
            },
            2 => circle(far),
            n => {
                let x = f64::from(n);
                EdgeDescription {
                    curve: EdgeCurve::Other,
                    length: 1.0,
                    start: [x, 0.0, 0.0],
                    end: [x, 1.0, 0.0],
                    middle: [x, 0.5, 0.0],
                }
            }
        })
        .collect();
    let vertices = (1..=8u32)
        .map(|n| match n {
            1 => [4.0, 4.0, 0.0],
            n => [f64::from(n), 0.0, 0.0],
        })
        .collect();
    Description {
        faces,
        edges,
        vertices,
    }
}

/// Answers every describe of a box on `kernel` with `whole`'s batch.
fn forge(kernel: &FakeKernel, whole: Description) {
    kernel.answer_instead(move |op| match op {
        Op::Describe {
            kind, first, count, ..
        } => {
            let from = (*first - 1) as usize;
            let slice = |n: usize| from.min(n)..(from + *count as usize).min(n);
            let mut batch = Description {
                faces: Vec::new(),
                edges: Vec::new(),
                vertices: Vec::new(),
            };
            let total = match kind {
                ElementKind::Face => {
                    batch.faces = whole.faces[slice(6)].to_vec();
                    6
                }
                ElementKind::Edge => {
                    batch.edges = whole.edges[slice(12)].to_vec();
                    12
                }
                _ => {
                    batch.vertices = whole.vertices[slice(8)].to_vec();
                    8
                }
            };
            Some(Reply::Described {
                total,
                description: batch,
            })
        }
        _ => None,
    });
}

fn forged(text: &str, whole: Description) -> (Recompute<FakeKernel>, Arc<Model>) {
    let engine = Recompute::new(FakeKernel::new(1).unwrap());
    forge(engine.kernel(), whole);
    let model = published(&engine, checked(text));
    (engine, model)
}

/// Each of `a` and `b`'s `center_to_element`, either way round, the same.
fn centre_to(engine: &Recompute<FakeKernel>, model: &Model, a: &str, b: &str) -> Option<Length> {
    let ab = distance(model, engine.kernel(), &pair(&name(a), &name(b)), None).unwrap();
    let ba = distance(model, engine.kernel(), &pair(&name(b), &name(a)), None).unwrap();
    assert_eq!(ab.center_to_element, ba.center_to_element, "{a} and {b}");
    ab.center_to_element
}

// ---- The nanodegree ----

#[test]
fn an_axis_a_fraction_of_a_nanodegree_off_is_parallel_below_half_and_null_above() {
    // Axes through (0, 0, 7), off x toward z by 0.45 and 0.499 nanodegrees (0 at the
    // nanodegree: parallel, 7 above the plane z = 0 and 7 from the line along x) and by
    // 0.501 and 0.55 (1: null), the plane and the line each.
    let ndeg = |n: f64| off_x(n * 1e-9);
    let p = [0.0, 0.0, 7.0];
    let (e, m) = forged(
        BOX,
        description(
            [
                (p, ndeg(0.45)),
                (p, ndeg(0.499)),
                (p, ndeg(0.501)),
                (p, ndeg(0.55)),
            ],
            [0.0; 3],
        ),
    );
    for (face, want) in [
        ("Face2", Some(mm(7))),
        ("Face3", Some(mm(7))),
        ("Face4", None),
        ("Face5", None),
    ] {
        assert_eq!(
            centre_to(&e, &m, face, "Face1"),
            want,
            "{face} and the plane"
        );
        assert_eq!(
            centre_to(&e, &m, face, "Edge1"),
            want,
            "{face} and the line"
        );
    }
}

#[test]
fn an_axis_turned_end_for_end_lying_in_the_plane_or_through_the_line_is_its_gap() {
    // Along −x through (0, 3, 4): 4 above the plane, 5 from the line (either sign of a
    // direction is the same axis). Along y through (2, 0, 0): in the plane, 0 from it, and
    // across the line's direction at right angles, null. Along x through (9, 0, 0): on the
    // line, 0, its point past the edge's end; in the plane, 0. Along −x through (−1e6, 6, 8),
    // a kilometre out: 8 and 10.
    let (e, m) = forged(
        BOX,
        description(
            [
                ([0.0, 3.0, 4.0], [-1.0, 0.0, 0.0]),
                ([2.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
                ([19.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
                ([-1e6, 6.0, 8.0], [-1.0, 0.0, 0.0]),
            ],
            [0.0; 3],
        ),
    );
    assert_eq!(centre_to(&e, &m, "Face2", "Face1"), Some(mm(4)));
    assert_eq!(centre_to(&e, &m, "Face2", "Edge1"), Some(mm(5)));
    assert_eq!(centre_to(&e, &m, "Face3", "Face1"), Some(mm(0)));
    assert_eq!(centre_to(&e, &m, "Face3", "Edge1"), None);
    assert_eq!(centre_to(&e, &m, "Face4", "Face1"), Some(mm(0)));
    assert_eq!(centre_to(&e, &m, "Face4", "Edge1"), Some(mm(0)));
    assert_eq!(centre_to(&e, &m, "Face5", "Face1"), Some(mm(8)));
    assert_eq!(centre_to(&e, &m, "Face5", "Edge1"), Some(mm(10)));
}

// ---- Which shape the point is measured on ----

#[test]
fn a_centre_is_measured_on_the_other_features_shape_either_way_round() {
    // Two boxes, both forged: First's Vertex1 (4, 4, 0) against Second's Face1, and the
    // other way round. The centre's request is on Second's shape, the one the least
    // distance's request holds for Second, whichever comes first; and its distance the
    // fake's from (1, 0, 0), 5.
    let engine = Recompute::new(FakeKernel::new(1).unwrap());
    forge(
        engine.kernel(),
        description([([0.0; 3], [1.0, 0.0, 0.0]); 4], [0.0; 3]),
    );
    let m = published(&engine, checked(TWO));
    let kernel = engine.kernel();
    for (a, b) in [
        ("aaaaaaaa.box.Vertex1", "bbbbbbbb.box.Face1"),
        ("bbbbbbbb.box.Face1", "aaaaaaaa.box.Vertex1"),
    ] {
        let before = kernel.calls().len();
        let d = distance(&m, kernel, &pair(a, b), None).unwrap();
        assert_eq!(d.center_to_element, Some(mm(5)), "{a} to {b}");
        let calls = kernel.calls()[before..].to_vec();
        let least = calls
            .iter()
            .find_map(|c| match &c.op {
                Op::Distance { shape, other, .. } => Some((shape.clone(), other.clone())),
                _ => None,
            })
            .unwrap();
        let second = if a.starts_with("bbbbbbbb") {
            least.0
        } else {
            least.1.unwrap()
        };
        let asked: Vec<_> = calls
            .iter()
            .filter_map(|c| match &c.op {
                Op::PointDistance { shape, .. } => Some(shape.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(asked, [second], "{a} to {b}");
        assert!(
            FakeKernel::describe(&asked[0]).is_some_and(|made| made.contains('5')),
            "{:?}",
            FakeKernel::describe(&asked[0])
        );
    }
}

// ---- A worker's reply that no pair has ----

#[test]
fn a_reply_to_the_centres_distance_no_pair_has_is_the_workers_fault_said_and_the_kernel_fine() {
    // Edge2, a circle centred (1, 3, 4), against Face1: the fake would say 5. A worker
    // answering its point distance with a distance below 0, NaN, a pair with an infinity,
    // no pair, or the wrong reply: a protocol error, naming both elements but not as the
    // request's fault (its `elements` not the parameter at fault), never a null, a 0, or the
    // least distance in its place.
    let bad = [
        Reply::Distance(silico_protocol::Distance {
            distance: -1.0,
            from: [1.0, 3.0, 4.0],
            to: [0.0; 3],
            solutions: 1,
        }),
        Reply::Distance(silico_protocol::Distance {
            distance: f64::NAN,
            from: [1.0, 3.0, 4.0],
            to: [0.0; 3],
            solutions: 1,
        }),
        Reply::Distance(silico_protocol::Distance {
            distance: 1.0,
            from: [1.0, 3.0, 4.0],
            to: [f64::INFINITY, 0.0, 0.0],
            solutions: 1,
        }),
        Reply::Distance(silico_protocol::Distance {
            distance: 1.0,
            from: [1.0, 3.0, 4.0],
            to: [0.0; 3],
            solutions: 0,
        }),
        Reply::Echo {
            data: silico_protocol::Blob::inline(vec![1, 2]),
        },
    ];
    for reply in bad {
        let engine = Recompute::new(FakeKernel::new(1).unwrap());
        forge(
            engine.kernel(),
            description([([0.0; 3], [1.0, 0.0, 0.0]); 4], [1.0, 3.0, 4.0]),
        );
        let m = published(&engine, checked(BOX));
        let sent = reply.clone();
        engine
            .kernel()
            .answer_instead(move |op| matches!(op, Op::PointDistance { .. }).then(|| sent.clone()));
        let refused = distance(
            &m,
            engine.kernel(),
            &pair(&name("Edge2"), &name("Face1")),
            None,
        )
        .unwrap_err();
        assert_eq!(refused.total(), 1, "{reply:?}: {:?}", refused.errors());
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::ProtocolError, "{reply:?}: {error}");
        assert_ne!(error.parameter(), Some("elements"), "{reply:?}: {error}");
        assert_eq!(
            error.elements(),
            [name("Edge2").as_str(), name("Face1").as_str()],
            "{reply:?}"
        );
        assert!(error.suggestion().is_some(), "{reply:?}: {error}");
        // The least distance alone still answers on the same kernel.
        let d = distance(
            &m,
            engine.kernel(),
            &pair(&name("Face6"), &name("Face1")),
            None,
        )
        .unwrap();
        assert_eq!(d.distance, mm(5));
    }
}

#[test]
fn a_reply_past_what_a_length_holds_is_an_internal_error_naming_both() {
    // A centre's distance of 1e300: finite, so no protocol error, but past any length: the
    // core's bug report, naming both, never a wrapped or saturated length.
    let engine = Recompute::new(FakeKernel::new(1).unwrap());
    forge(
        engine.kernel(),
        description([([0.0; 3], [1.0, 0.0, 0.0]); 4], [1.0, 3.0, 4.0]),
    );
    let m = published(&engine, checked(BOX));
    engine.kernel().answer_instead(|op| {
        matches!(op, Op::PointDistance { .. }).then(|| {
            Reply::Distance(silico_protocol::Distance {
                distance: 1e300,
                from: [1.0, 3.0, 4.0],
                to: [1e300, 0.0, 0.0],
                solutions: 1,
            })
        })
    });
    let refused = distance(
        &m,
        engine.kernel(),
        &pair(&name("Vertex1"), &name("Face1")),
        None,
    )
    .unwrap_err();
    let error = &refused.errors()[0];
    assert_eq!(error.code(), ErrorCode::Internal, "{error}");
    assert_eq!(
        error.elements(),
        [name("Vertex1").as_str(), name("Face1").as_str()]
    );
}

// ---- A centre past the point's bound ----

#[test]
fn a_circles_centre_past_the_points_bound_is_refused_naming_the_elements_never_asked() {
    // A description whose circle is centred 2e12 out (no solid the gate admits has one,
    // so this is the core's guard): refused before the kernel is asked, naming both
    // elements, and saying which is at fault in the request's own terms, not a `point` the
    // request never had.
    //
    // Fixed (an `internal` error, its cause the point's refusal); was: the request's own
    // refusal of the point (`check_point_distance`) passed through `named` as the pair's
    // fault: `invalid_input`, parameter `elements`, "point
    // [2000000000000.0, 0.0, 0.0] must be finite and within 1000000000000 mm of the
    // origin", suggestion "change the value the message names". The agent named two
    // elements and no point; there's no value of its own to change, and a centre past any
    // solid the gate admits is the core's bug (`internal`, SILICO_BUG, as `too_far` says a
    // length past what one holds), not the request's. Unreachable from a gated document
    // today (a circle's centre is a sketch point, an axis, or a primitive's, each within
    // the gate's kilometre), so low.
    let (e, m) = forged(
        BOX,
        description([([0.0; 3], [1.0, 0.0, 0.0]); 4], [2e12, 0.0, 0.0]),
    );
    let before = e.kernel().calls().len();
    let result = distance(&m, e.kernel(), &pair(&name("Edge2"), &name("Face1")), None);
    let asked = e.kernel().calls()[before..]
        .iter()
        .filter(|c| matches!(c.op, Op::PointDistance { .. }))
        .count();
    assert_eq!(asked, 0);
    match result {
        Ok(d) => panic!("answered: {d:?}"),
        Err(refused) => {
            let error = &refused.errors()[0];
            assert_eq!(
                error.elements(),
                [name("Edge2").as_str(), name("Face1").as_str()],
                "{error}"
            );
            assert!(
                !error.message().contains("point ["),
                "the message names a `point` the request never had: {error}"
            );
            assert_eq!(error.code(), ErrorCode::Internal, "{error}");
        }
    }
}

// ---- The session ----

/// A session's `distance` of Vertex1 to Face1 of the forged box, of its published model, its
/// centre's request held in the kernel: the running read.
fn held_in_session(
    f: &super::listing::Fixture,
    session: &silico_core::command::SessionId,
) -> silico_core::command::Running<silico_core::command::measure::DistanceAnswer> {
    // Built and published first.
    assert!(
        answer(
            f.sessions
                .elements(session, ModelChoice::Current, None, None, 10)
        )
        .result
        .is_ok()
    );
    let reading = f.sessions.distance(
        session,
        ModelChoice::Published,
        pair(&name("Vertex1"), &name("Face1")),
        None,
    );
    // Its least distance answered, its centre's distance held.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !f
        .kernel
        .calls()
        .iter()
        .any(|c| matches!(c.op, Op::PointDistance { .. }))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "no point distance asked"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    match reading.wait_timeout(Duration::from_millis(200)) {
        Ok(response) => panic!("answered while held: {:?}", response.result),
        Err(running) => running,
    }
}

#[test]
fn a_centres_distance_held_while_its_read_is_cancelled_answers_cancelled() {
    let f = fixture();
    forge(
        &f.kernel,
        description([([0.0; 3], [1.0, 0.0, 0.0]); 4], [0.0; 3]),
    );
    let s = f.open("part.silico", BOX);
    let hold = f.kernel.hold(|op| matches!(op, Op::PointDistance { .. }));
    let reading = held_in_session(&f, &s);
    let stop = f.sessions.cancel(&s, "distance").result.unwrap();
    assert!(stop.stopped);
    let response = reading.wait_timeout(Duration::from_secs(10)).ok().unwrap();
    let refused = response.result.unwrap_err();
    assert_eq!(refused.errors()[0].code(), ErrorCode::Cancelled);
    hold.release();
    // The session goes on: the same distance, unheld, is the fake's 5.
    let again = answer(f.sessions.distance(
        &s,
        ModelChoice::Current,
        pair(&name("Vertex1"), &name("Face1")),
        None,
    ));
    assert_eq!(again.result.unwrap().center_to_element, Some(mm(5)));
}

#[test]
fn a_centres_distance_held_while_its_session_closes_says_the_session_closed() {
    let f = fixture();
    forge(
        &f.kernel,
        description([([0.0; 3], [1.0, 0.0, 0.0]); 4], [0.0; 3]),
    );
    let s = f.open("part.silico", BOX);
    let hold = f.kernel.hold(|op| matches!(op, Op::PointDistance { .. }));
    let reading = held_in_session(&f, &s);
    assert!(
        f.sessions
            .close(&s)
            .wait_timeout(Duration::from_secs(5))
            .is_ok_and(|r| r.result.is_ok())
    );
    let response = reading.wait_timeout(Duration::from_secs(10)).ok().unwrap();
    let refused = response.result.unwrap_err();
    assert_eq!(
        refused.errors()[0].message(),
        "the session closed before the kernel measured the distance"
    );
    hold.release();
}
