//! `command::render` through the render worker (docs/design/m4.6-render.md, Tests, the
//! result): the example's body, rebuilt on the fake kernel (its mesh a triangle a face on
//! the box's corners), drawn by the worker on lavapipe, its PNG written where asked and read
//! back by the `png` crate at the size asked.

use std::path::Path;
use std::sync::Arc;

use silico_core::command::render::{Render, RenderOn, render};
use silico_core::command::{self};
use silico_core::fake::FakeKernel;
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
use silico_core::recompute::Recompute;
use silico_core::render::{RenderClient, RenderSettings};
use silico_core::views::NamedView;

use crate::common::{WAIT, WORKER};

const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
    {"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12},
    {"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}
  ]
}
"#;

#[test]
fn a_render_to_a_file_is_a_png_of_the_size_asked_and_the_result_says_its_bytes() {
    let dir = std::env::temp_dir().join(format!("silico-render-to-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let engine = Recompute::new(Arc::clone(&kernel));
    let model = command::rebuild(&engine, command::gate(EXAMPLE.as_bytes()).unwrap())
        .unwrap()
        .unwrap();
    let meshes = MeshCache::new(MESH_CACHE_BYTES);
    let renderer = RenderClient::start(Path::new(WORKER), RenderSettings::default()).unwrap();
    let request = Render {
        view: NamedView::named("top_front_right"),
        camera: None,
        width: 120,
        height: 90,
        highlight: vec![],
        // Spelt otherwise than through its links on every system, so a result that resolved
        // it would differ.
        to: Some(dir.join(".").join("view.png")),
        replace: false,
    };
    let done = render(
        &model,
        &request,
        &RenderOn {
            kernel: &kernel,
            meshes: &meshes,
            renderer: &renderer,
            workspace: Some(&silico_core::file::Workspace::new(&dir).unwrap()),
            held: &[],
            settle: &|| Ok(()),
        },
    )
    .unwrap();
    let path = done.result.path.clone().unwrap();
    // The request's own bytes (#182: a result's path is the request's), not the path through
    // its links (on macOS, the temporary directory's /var is a link to /private/var), and not
    // `Path`'s equality, which takes `a//b` and `a/./b` for `a/b`: an `OsStr`'s is its bytes'.
    assert_eq!(path.as_os_str(), request.to.as_ref().unwrap().as_os_str());
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes, done.png);
    assert_eq!(done.result.png_bytes, bytes.len() as u64);
    let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    let reader = decoder.read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (120, 90));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_render_through_a_session_cancelled_while_it_runs_answers_cancelled_and_stops_its_worker() {
    // The test worker hangs on a 1 x 7 scene (test_scenes::HANGS). Through the session, the
    // read's cancel reaches the render: it answers cancelled at once, its worker killed, and
    // the next render through the session draws.
    use silico_core::cache::Cache;
    use silico_core::command::{ModelChoice, Sessions};
    use std::time::{Duration, Instant};
    let dir = std::env::temp_dir().join(format!("silico-render-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret");
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let client = Arc::new(
        RenderClient::start(
            std::path::Path::new(crate::common::TEST_WORKER),
            RenderSettings::default(),
        )
        .unwrap(),
    );
    let sessions = Sessions::with_cache_opener(Arc::clone(&kernel), move |cache| {
        Cache::with_secret(cache, &secret)
    })
    .rendering(Arc::clone(&client));
    let file = dir.join("example.silico");
    std::fs::write(&file, EXAMPLE).unwrap();
    let opened = sessions.open(&file, false).wait();
    let s = opened.session.unwrap().id;
    let hangs = |(width, height): (u32, u32)| Render {
        view: NamedView::named("front"),
        camera: None,
        width,
        height,
        highlight: vec![],
        to: None,
        replace: false,
    };
    let running = sessions.render(
        &s,
        ModelChoice::Current,
        hangs(silico_render::test_scenes::HANGS),
    );
    let deadline = Instant::now() + WAIT;
    while client.pid().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = client.pid().expect("the render never started");
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    assert!(running.cancel());
    let answer = running.wait();
    let problems = answer.result.unwrap_err();
    assert_eq!(
        problems.errors()[0].code(),
        silico_core::ErrorCode::Cancelled
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    assert!(crate::common::eventually(
        crate::common::CANCEL_KILLS,
        || { !crate::common::process_exists(pid) }
    ));
    let drawn = sessions
        .render(&s, ModelChoice::Current, hangs((40, 30)))
        .wait();
    assert!(drawn.result.is_ok(), "{:?}", drawn.result);
    drop(sessions);
    let _ = std::fs::remove_dir_all(&dir);
}
