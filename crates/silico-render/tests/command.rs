//! `command::render` through the render worker (docs/design/m4.6-render.md, Tests, the
//! result): the example's body, rebuilt on the fake kernel (its mesh a triangle a face on
//! the box's corners), drawn by the worker on lavapipe, its PNG written where asked and read
//! back by the `png` crate at the size asked.

use std::path::Path;
use std::sync::Arc;

use silico_core::command::render::{Render, RenderOn, render};
use silico_core::command::{self};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Kernel;
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
use silico_core::recompute::Recompute;
use silico_core::render::{RenderClient, RenderSettings};
use silico_core::views::NamedView;

use crate::common::WORKER;

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
        to: Some(dir.join("view.png")),
        replace: false,
    };
    let done = render(
        &model,
        &request,
        &RenderOn {
            kernel: &kernel,
            generation: kernel.generation(),
            meshes: &meshes,
            renderer: &renderer,
            workspace: Some(&dir),
            held: &[],
        },
    )
    .unwrap();
    let path = done.result.path.clone().unwrap();
    assert_eq!(path, std::fs::canonicalize(dir.join("view.png")).unwrap());
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes, done.png);
    assert_eq!(done.result.png_bytes, bytes.len() as u64);
    let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    let reader = decoder.read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (120, 90));
    let _ = std::fs::remove_dir_all(&dir);
}
