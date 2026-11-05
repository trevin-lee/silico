//! `Sessions::meshes` and `meshes` by name (docs/design/m5.0-app.md, section 7, `meshes`,
//! M5.5): a session's current or published model meshed on the read's own kernel client,
//! through the meshes the sessions keep; by name, the typed answer's envelope byte for byte;
//! `held` given back as a result wrote it; the size's limits; a suppressed feature; the
//! published reads' limit, `meshes` among them; and `cancel` naming it. The fake kernel,
//! whose record says what it was asked to mesh. Expected values by hand, from the example:
//! the cut m3tq8v0z, its box (0, 0, −1) to (40, 20, 11), 9 faces and 15 edges on the fake (a
//! triangle a face, two points an edge); a front view 80 mm high over 1,000 pixels has a half
//! pixel of 0.04, so its level is 2⁻⁵.

use std::sync::{Arc, Mutex, mpsc};

use super::example::EXAMPLE;
use super::listing::{Fixture, HANG, TWO_BODIES, answer, fixture};
use silico_core::ErrorCode;
use silico_core::command::meshes::{Meshes, MeshesRead};
use silico_core::command::wire::{Layout, response_text};
use silico_core::command::{
    Call, Cancellable, MAX_PUBLISHED_READS, ModelChoice, ReadCommand, Response, SessionCommand,
    SessionId,
};
use silico_core::kernel::Op;
use silico_protocol::{Camera, Projection};

/// Orthographic, from −y onto the cut's centre, `view_height` high.
fn front(view_height: f64) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

fn of(camera: Camera, width: u32, height: u32) -> Meshes {
    Meshes {
        view: None,
        camera: Some(camera),
        width,
        height,
        features: None,
        held: vec![],
    }
}

/// The front camera 80 mm high as a request writes it.
const CAMERA: &str = r#"{"projection": "orthographic", "eye": [20, -100, 5], "target": [20, 10, 5], "up": [0, 0, 1], "view_height": 80, "field_of_view": null}"#;

/// A request of s1's `model` at 1,000 pixels square, holding `held` (JSON text).
fn request(model: &str, held: &str) -> String {
    format!(
        r#"{{"session": "s1", "model": "{model}", "view": null, "camera": {CAMERA}, "width": 1000, "height": 1000, "features": null, "held": {held}}}"#
    )
}

/// How many meshes the kernel has been asked for.
fn meshed(f: &Fixture) -> usize {
    f.kernel
        .calls()
        .iter()
        .filter(|c| matches!(c.op, Op::Tessellate { .. }))
        .count()
}

/// `name` called on `arguments`: its answer's envelope, on one line.
fn called(f: &Fixture, name: &str, arguments: &str) -> (bool, String) {
    match f.sessions.call(name, arguments) {
        Call::Running(calling) => match calling.wait_timeout(HANG) {
            Ok(answered) => (answered.ok, answered.text),
            Err(_) => panic!("{name} didn't answer"),
        },
        Call::Unknown => panic!("{name} isn't a command"),
    }
}

fn read_of(response: Response<MeshesRead>) -> MeshesRead {
    match response.result {
        Ok(read) => read,
        Err(problems) => panic!("{:?}", problems.errors()),
    }
}

#[test]
fn a_sessions_current_and_published_models_are_meshed_each_face_and_edge_named() {
    let f = fixture();
    let s = f.open("example.silico", EXAMPLE);
    let current = answer(
        f.sessions
            .meshes(&s, ModelChoice::Current, of(front(80.0), 1000, 1000)),
    );
    assert_eq!(
        current.model.as_ref().map(|m| m.of),
        Some(ModelChoice::Current)
    );
    let read = read_of(current);
    assert_eq!(read.bodies.len(), 1);
    let body = &read.bodies[0];
    assert_eq!(body.feature.as_str(), "m3tq8v0z");
    assert_eq!(body.body, Some(1));
    assert_eq!((body.level, body.deflection), (0.03125, 0.03125));
    assert_eq!(body.serves, [0.03125, 0.0625]);
    assert_eq!(body.bounds, Some([[0.0, 0.0, -1.0], [40.0, 20.0, 11.0]]));
    assert!(!body.held);
    assert_eq!((body.faces.len(), body.edges.len()), (9, 15));
    assert!(body.faces.iter().all(|face| face.count == Some(1)));
    assert!(body.edges.iter().all(|edge| edge.count == Some(2)));
    // What a response too long for one body would name: a camera's height.
    assert_eq!(read.coarser, "height");
    assert_eq!(meshed(&f), 1);
    // The published model, the same revision's: the same mesh, from the sessions' meshes,
    // the kernel not asked again.
    let published = answer(f.sessions.meshes(
        &s,
        ModelChoice::Published,
        of(front(80.0), 1000, 1000),
    ));
    assert_eq!(
        published.model.as_ref().map(|m| m.of),
        Some(ModelChoice::Published)
    );
    let again = read_of(published);
    assert!(Arc::ptr_eq(
        again.bodies[0].mesh.as_ref().unwrap(),
        body.mesh.as_ref().unwrap()
    ));
    assert_eq!(meshed(&f), 1);
}

#[test]
fn a_named_views_side_that_bound_its_fit_is_the_one_a_body_too_long_names() {
    // The cut from the front is 40 mm wide and 12 high: on a square image, or a tall one,
    // its width binds the fit; on an image 100 times wider than high, its height (40 · 10 /
    // 1,000 = 0.4 against 12).
    let f = fixture();
    let s = f.open("example.silico", EXAMPLE);
    for (width, height, side) in [
        (100, 100, "width"),
        (100, 1000, "width"),
        (1000, 10, "height"),
    ] {
        let read = read_of(answer(f.sessions.meshes(
            &s,
            ModelChoice::Current,
            Meshes {
                view: Some(silico_core::views::NamedView::Front),
                camera: None,
                width,
                height,
                features: None,
                held: vec![],
            },
        )));
        assert_eq!(read.coarser, side, "{width} by {height}");
    }
}

/// The fixture's sessions drawing with a fake renderer, and the example opened.
fn rendering() -> (Fixture, SessionId) {
    let Fixture {
        dir,
        kernel,
        sessions,
    } = fixture();
    let f = Fixture {
        dir,
        kernel,
        sessions: sessions.rendering(silico_core::fake::FakeRenderer::new()),
    };
    let s = f.open("example.silico", EXAMPLE);
    (f, s)
}

#[test]
fn a_body_rendered_and_read_as_meshes_at_one_view_is_meshed_once_whichever_comes_first() {
    // The sessions' one cache (M4.6's R1): the front, 80 mm over 1,000 pixels, 2⁻⁵.
    let render = |f: &Fixture, s: &SessionId| {
        answer(f.sessions.render(
            s,
            ModelChoice::Current,
            silico_core::command::render::Render {
                view: None,
                camera: Some(front(80.0)),
                width: 1000,
                height: 1000,
                highlight: vec![],
                to: None,
                replace: false,
            },
        ))
        .result
        .unwrap();
    };
    let read = |f: &Fixture, s: &SessionId| {
        read_of(answer(f.sessions.meshes(
            s,
            ModelChoice::Current,
            of(front(80.0), 1000, 1000),
        )))
    };
    let (f, s) = rendering();
    render(&f, &s);
    assert_eq!(meshed(&f), 1);
    assert_eq!(read(&f, &s).bodies[0].deflection, 0.03125);
    assert_eq!(meshed(&f), 1);
    let (f, s) = rendering();
    assert_eq!(read(&f, &s).bodies[0].deflection, 0.03125);
    assert_eq!(meshed(&f), 1);
    render(&f, &s);
    assert_eq!(meshed(&f), 1);
}

#[test]
fn meshes_by_name_answers_the_typed_reads_envelope_byte_for_byte() {
    let f = fixture();
    let s = f.open("example.silico", EXAMPLE);
    for model in [ModelChoice::Current, ModelChoice::Published] {
        let typed = answer(f.sessions.meshes(&s, model, of(front(80.0), 1000, 1000)));
        let (ok, text) = called(&f, "meshes", &request(model.as_str(), "[]"));
        assert!(ok, "{text}");
        assert_eq!(text, response_text(&typed, Layout::OneLine), "{model:?}");
    }
    // A refusal by name is the core's refused envelope, naming the session.
    let (ok, text) = called(&f, "meshes", &request("current", "null"));
    assert!(!ok);
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["session"]["id"], "s1", "{envelope}");
    assert_eq!(envelope["errors"][0]["parameter"], "held", "{envelope}");
}

#[test]
fn held_given_back_as_the_result_wrote_it_answers_the_body_named_with_no_geometry() {
    let f = fixture();
    f.open("example.silico", EXAMPLE);
    let (ok, first) = called(&f, "meshes", &request("current", "[]"));
    assert!(ok, "{first}");
    let first: serde_json::Value = serde_json::from_str(&first).unwrap();
    let body = &first["result"]["bodies"][0];
    assert_eq!(body["held"], false);
    // The fake's mesh, as written: three corners of the box, one triangle a face, each
    // edge its box's diagonal.
    assert_eq!(
        body["positions"],
        serde_json::json!([[0, 0, -1], [40, 0, -1], [0, 20, 11]])
    );
    assert_eq!(
        body["normals"],
        serde_json::json!([[0, 0, 1], [0, 0, 1], [0, 0, 1]])
    );
    assert_eq!(body["triangles"], serde_json::json!(vec![[0, 1, 2]; 9]));
    assert_eq!(
        body["edge_points"],
        serde_json::json!([[0, 0, -1], [40, 20, 11]].repeat(15))
    );
    // Each vertex the box's high corner, one for each the map names, each named once, with
    // its name and stability alone.
    let vertices = body["vertices"].as_array().unwrap();
    assert!(!vertices.is_empty());
    assert_eq!(
        body["vertex_points"],
        serde_json::json!(vec![[40, 20, 11]; vertices.len()])
    );
    let mut names = std::collections::BTreeSet::new();
    for vertex in vertices {
        assert_eq!(
            vertex.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["name", "stable"]
        );
        let name = vertex["name"].as_str().unwrap();
        assert!(name.contains("Vertex") && names.insert(name), "{vertex}");
    }
    let held = format!(
        r#"[{{"key": {}, "deflection": {}}}]"#,
        body["key"], body["deflection"]
    );
    let (ok, second) = called(&f, "meshes", &request("current", &held));
    assert!(ok, "{second}");
    let second: serde_json::Value = serde_json::from_str(&second).unwrap();
    let again = &second["result"]["bodies"][0];
    assert_eq!(again["held"], true);
    for array in [
        "positions",
        "normals",
        "triangles",
        "edge_points",
        "vertex_points",
    ] {
        assert_eq!(again[array], serde_json::json!([]), "{array}");
    }
    // The vertices, named still, as they were.
    assert_eq!(again["vertices"], body["vertices"]);
    // The same names, each count null.
    for (list, count) in [("faces", "triangles"), ("edges", "points")] {
        let names = |b: &serde_json::Value| -> Vec<serde_json::Value> {
            b[list]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["name"].clone())
                .collect()
        };
        assert_eq!(names(again), names(body), "{list}");
        assert!(
            again[list]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e[count].is_null()),
            "{list}"
        );
    }
    // Everything else as it was.
    for field in [
        "feature",
        "body",
        "key",
        "level",
        "deflection",
        "box",
        "serves",
    ] {
        assert_eq!(again[field], body[field], "{field}");
    }
    assert_eq!(meshed(&f), 1);
    // Held at another deflection, or under another key, it isn't held.
    let other = format!(
        r#"[{{"key": {}, "deflection": 0.0625}}, {{"key": "{}", "deflection": {}}}]"#,
        body["key"],
        "0".repeat(64),
        body["deflection"]
    );
    let (ok, third) = called(&f, "meshes", &request("current", &other));
    assert!(ok, "{third}");
    let third: serde_json::Value = serde_json::from_str(&third).unwrap();
    assert_eq!(third["result"]["bodies"][0]["held"], false);
    assert_eq!(third["result"]["bodies"][0], *body);
}

#[test]
fn a_size_past_either_end_is_refused_naming_its_side_and_the_ends_are_taken() {
    let f = fixture();
    let s = f.open("example.silico", EXAMPLE);
    for (width, height, parameter, message) in [
        (
            0,
            100,
            "width",
            "width must be from 1 to 4096 pixels, got 0",
        ),
        (
            100,
            4097,
            "height",
            "height must be from 1 to 4096 pixels, got 4097",
        ),
    ] {
        let refused = answer(f.sessions.meshes(
            &s,
            ModelChoice::Current,
            of(front(80.0), width, height),
        ));
        let problems = refused.result.unwrap_err();
        assert_eq!(problems.total(), 1);
        let e = &problems.errors()[0];
        assert_eq!(
            (e.code(), e.parameter(), e.message()),
            (ErrorCode::InvalidInput, Some(parameter), message)
        );
    }
    assert_eq!(meshed(&f), 0);
    // 4,096 by 1: 80 mm over one pixel, a half pixel of 40, level 32.
    let read = read_of(answer(f.sessions.meshes(
        &s,
        ModelChoice::Current,
        of(front(80.0), 4096, 1),
    )));
    assert_eq!(read.bodies[0].level, 32.0);
}

#[test]
fn a_suppressed_feature_named_is_refused_as_the_contract_writes_it() {
    let f = fixture();
    let text = TWO_BODIES.replace(
        r#""name": "Second", "#,
        r#""name": "Second", "suppressed": true, "#,
    );
    assert_ne!(text, TWO_BODIES);
    f.open("two.silico", &text);
    let arguments = r#"{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 100, "height": 100, "features": ["bbbbbbbb"], "held": []}"#;
    let (ok, text) = called(&f, "meshes", arguments);
    assert!(!ok);
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    let example: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/contract/commands/examples/meshes/refused-suppressed.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        envelope["errors"],
        serde_json::json!([example]),
        "{envelope}"
    );
    assert_eq!(envelope["total"], 1);
    // Its words, by hand.
    let e = &envelope["errors"][0];
    assert_eq!(
        e["message"],
        "bbbbbbbb is suppressed, so it has no solid to mesh"
    );
    assert_eq!(
        e["suggestion"],
        "unsuppress it first, or name a feature that isn't suppressed"
    );
    assert_eq!(
        (&e["parameter"], &e["feature"]),
        (
            &serde_json::json!("features.0"),
            &serde_json::json!("bbbbbbbb")
        )
    );
}

#[test]
fn a_ninth_published_read_a_cached_meshes_is_refused_and_answers_once_one_ends() {
    let f = fixture();
    let s = f.open("example.silico", EXAMPLE);
    // The level kept: a published meshes of it asks the kernel nothing.
    read_of(answer(f.sessions.meshes(
        &s,
        ModelChoice::Current,
        of(front(80.0), 1000, 1000),
    )));
    assert_eq!(meshed(&f), 1);
    let (started, starts) = mpsc::channel::<()>();
    // Eight held, each by its own release, the last of them a meshes.
    let mut releases = Vec::new();
    let held: Vec<_> = (0..MAX_PUBLISHED_READS)
        .map(|at| {
            let (release, released) = mpsc::channel::<()>();
            releases.push(release);
            let released = Mutex::new(released);
            let started = started.clone();
            let command = if at + 1 == MAX_PUBLISHED_READS {
                ReadCommand::Meshes
            } else {
                ReadCommand::Check
            };
            f.sessions
                .read_model(&s, command, ModelChoice::Published, move |_| {
                    started.send(()).unwrap();
                    let _ = released.lock().unwrap().recv();
                    Ok(())
                })
        })
        .collect();
    for _ in 0..MAX_PUBLISHED_READS {
        starts.recv_timeout(HANG).unwrap();
    }
    let reads = f.sessions.status(&s).result.unwrap().reads;
    assert_eq!(reads.len(), MAX_PUBLISHED_READS);
    assert_eq!(reads.last(), Some(&ReadCommand::Meshes));
    // By name too, as status writes it.
    let (_, status) = called(&f, "status", r#"{"session": "s1"}"#);
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["result"]["reads"][7], "meshes", "{status}");
    let past = answer(
        f.sessions
            .meshes(&s, ModelChoice::Published, of(front(80.0), 1000, 1000)),
    );
    let problems = past.result.unwrap_err();
    assert_eq!(
        problems.errors()[0].message(),
        format!(
            "{MAX_PUBLISHED_READS} published reads are running in session {s}, the most one session runs at once"
        )
    );
    // The current model's, queued, isn't one of them.
    read_of(answer(f.sessions.meshes(
        &s,
        ModelChoice::Current,
        of(front(80.0), 1000, 1000),
    )));
    // One released: its slot frees, and a published meshes answers while the seven others
    // still hold theirs.
    let mut held = held.into_iter();
    releases.remove(0).send(()).unwrap();
    answer(held.next().unwrap()).result.unwrap();
    let read = read_of(answer(f.sessions.meshes(
        &s,
        ModelChoice::Published,
        of(front(80.0), 1000, 1000),
    )));
    assert_eq!(read.bodies[0].level, 0.03125);
    assert_eq!(meshed(&f), 1);
    let reads = f.sessions.status(&s).result.unwrap().reads;
    assert_eq!(reads.len(), MAX_PUBLISHED_READS - 1);
    // The rest released.
    drop(releases);
    for read in held {
        answer(read).result.unwrap();
    }
}

#[test]
fn cancel_naming_meshes_stops_a_published_read_meshing_and_it_answers_cancelled() {
    let f = fixture();
    let s: SessionId = f.open("example.silico", EXAMPLE);
    // The model built, then a level not kept, held in the kernel.
    read_of(answer(f.sessions.meshes(
        &s,
        ModelChoice::Current,
        of(front(80.0), 1000, 1000),
    )));
    let hold = f.kernel.hold(|op| op.name() == "Tessellate");
    let running = f
        .sessions
        .meshes(&s, ModelChoice::Published, of(front(160.0), 1000, 1000));
    assert!(f.kernel.wait_running(1, HANG));
    let status = f.sessions.status(&s).result.unwrap();
    assert_eq!(status.reads, [ReadCommand::Meshes]);
    // `cancel` by name, as an agent sends it.
    let (ok, text) = called(&f, "cancel", r#"{"session": "s1", "command": "meshes"}"#);
    assert!(ok, "{text}");
    let cancelled: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        cancelled["result"],
        serde_json::json!({"command": "meshes", "stopped": true})
    );
    let answered = match running.wait_timeout(HANG) {
        Ok(answered) => answered,
        Err(_) => panic!("the cancelled read didn't answer"),
    };
    let problems = answered.result.unwrap_err();
    assert_eq!(problems.errors()[0].code(), ErrorCode::Cancelled);
    hold.release();
    // Typed, it's the same name: with nothing running, refused saying so.
    let nothing = f
        .sessions
        .cancel(&s, SessionCommand::Meshes)
        .result
        .unwrap_err();
    assert_eq!(
        (
            nothing.errors()[0].message(),
            nothing.errors()[0].parameter()
        ),
        ("no meshes is running in session s1", Some("command"))
    );
    assert_eq!(Cancellable::from(SessionCommand::Meshes).as_str(), "meshes");
}
