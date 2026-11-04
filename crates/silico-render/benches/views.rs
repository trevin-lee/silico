//! A view change's cost (docs/design/m5.0-app.md, section 10): a corpus part drawn through
//! the worker's renderer once, then from cameras orbiting it, each render timed, so an orbit
//! that rebuilds every face's mesh shows against one that moves the bodies. The part is NIST's
//! CTC 02 (the corpus's largest, 664 faces) in CI's bench, alone and as ten copies (6,640
//! faces), meshed by a kernel worker at half a pixel of a 1080-pixel view of it, as the
//! study measured it (docs/study/viewport.md, section 5). Each line gives a render's whole time
//! and, of a view change, the CPU's part placing the faces and edges for the view (each
//! face's anchor and transform, its instance, the edges' strips: `Renderer::placed`).
//!
//! Then the ten copies at 1920 by 1080 from inside the first copy's box, the eye at its
//! centre turning 12 degrees a view (60°): what can have no pixel not placed, the faces
//! about the eye whose points cross the view's pyramid cut and uploaded again for it, the
//! rest moved as on the orbit; then the last view again, still (`inside` lines).
//!
//! Recorded, never budgeted: it fails on no time. Run with
//! `SILICO_KERNEL_WORKER=target/release/silico-kernel-worker SILICO_BENCH_BREP=part.brep
//! pixi run cargo bench -p silico-render --bench views`, the B-rep Silico's (as
//! `silico-brep-convert` writes it from the corpus's), after
//! `pixi run cargo build --release -p silico-kernel --bin silico-kernel-worker` (bench.yml's
//! `views`).

use std::error::Error;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use silico_core::kernel::{Blob, KernelWorker, ShapeData};
use silico_protocol::{Camera, Mesh, Projection, RenderBody, RenderScene};
use silico_render::render::Renderer;
use silico_render::worker::RenderAdapter;

/// The view changes timed for each scene and size, after its first render.
const VIEWS: usize = 30;
/// The image sizes: one small enough that the rasterizer's share is small, and a viewport's.
const SIZES: [(u32, u32); 2] = [(256, 256), (1920, 1080)];

fn main() -> Result<(), Box<dyn Error>> {
    silico_render::lavapipe::reexec_with()?;
    let var = |name: &str| {
        std::env::var_os(name).map(PathBuf::from).ok_or(format!(
            "{name} isn't set: the bench's module doc says how to run it"
        ))
    };
    let (worker, brep) = (var("SILICO_KERNEL_WORKER")?, var("SILICO_BENCH_BREP")?);
    let mut kernel = KernelWorker::start_worker(&worker)?;
    let shape = ShapeData::checked(Blob::inline(std::fs::read(&brep)?), true);
    let (low, high) = bounds(&kernel.tessellate(&shape, 1.0)?);
    let extent = (0..3).map(|k| high[k] - low[k]).fold(0.0, f64::max);
    // Half a pixel of a view that fits the part's widest side into 1080 pixels.
    let mesh = kernel.tessellate(&shape, extent / 1080.0 / 2.0)?;
    let adapters = silico_render::adapters(RenderAdapter::Software)?;
    let adapter = &adapters[0];
    let (device, queue) = silico_render::render::device(adapter)?;
    let mut renderer = Renderer::new(adapter, device, queue, silico_render::render::OUTPUT_FORMAT)?;
    let mut out = String::new();
    out += &format!("adapter {}\n", adapter.get_info().name);
    out += &format!(
        "part {} faces {} triangles {} edges\n",
        mesh.face_triangles.len(),
        mesh.triangles.len(),
        mesh.edge_point_counts.len()
    );
    for copies in [1usize, 10] {
        for (width, height) in SIZES {
            let bodies = copies_of(&mesh, copies, extent);
            let (target, reach) = centre_and_reach(&bodies);
            let scene = |view: usize| RenderScene {
                bodies: bodies.clone(),
                camera: orbit(target, reach, view),
                width,
                height,
                ids: false,
            };
            let first = time(&mut renderer, &scene(0))?;
            let uploaded = renderer.uploads();
            let mut changes = Vec::with_capacity(VIEWS);
            let mut placed = Vec::with_capacity(VIEWS);
            for view in 1..=VIEWS {
                changes.push(time(&mut renderer, &scene(view))?);
                placed.push(renderer.placed());
            }
            placed.sort();
            let reuploaded = renderer.uploads() - uploaded;
            changes.sort();
            let ms = |d: Duration| d.as_secs_f64() * 1e3;
            out += &format!(
                "copies {copies} size {width}x{height} first_ms {:.2} view_change_median_ms {:.2} view_change_p95_ms {:.2} placing_median_ms {:.3} placing_p95_ms {:.3} meshes_uploaded_by_view_changes {reuploaded}\n",
                ms(first),
                ms(changes[VIEWS / 2]),
                ms(changes[VIEWS * 95 / 100]),
                ms(placed[VIEWS / 2]),
                ms(placed[VIEWS * 95 / 100]),
            );
        }
    }
    out += &inside(&mut renderer, &mesh, extent)?;
    std::io::stdout().write_all(out.as_bytes())?;
    Ok(())
}

/// Ten copies of the part at 1920 by 1080 from inside the first copy's box, turning: each
/// view change's time, its placing (the cut faces' cutting and upload among it), and how
/// many face meshes each uploads.
fn inside(renderer: &mut Renderer, mesh: &Mesh, extent: f64) -> Result<String, Box<dyn Error>> {
    let bodies = copies_of(mesh, 10, extent);
    let (low, high) = bounds(&bodies[0].mesh);
    let eye: [f64; 3] = std::array::from_fn(|k| (low[k] + high[k]) / 2.0);
    let scene = |view: usize| {
        let angle = (view as f64 * 12.0).to_radians();
        RenderScene {
            bodies: bodies.clone(),
            camera: Camera {
                projection: Projection::Perspective {
                    field_of_view: 60.0,
                },
                eye,
                target: [
                    eye[0] + libm::cos(angle),
                    eye[1] + libm::sin(angle),
                    eye[2] + 0.3,
                ],
                up: [0.0, 0.0, 1.0],
            },
            width: 1920,
            height: 1080,
            ids: false,
        }
    };
    time(renderer, &scene(0))?;
    let (mut changes, mut placed, mut uploads) = (vec![], vec![], vec![]);
    for view in 1..=VIEWS {
        let before = renderer.uploads();
        changes.push(time(renderer, &scene(view))?);
        placed.push(renderer.placed());
        uploads.push(renderer.uploads() - before);
    }
    // The last view again, still: nothing to cut again, nothing uploaded.
    let (mut still, mut still_uploads) = (vec![], 0);
    for _ in 0..VIEWS {
        let before = renderer.uploads();
        time(renderer, &scene(VIEWS))?;
        still.push(renderer.placed());
        still_uploads += renderer.uploads() - before;
    }
    changes.sort();
    placed.sort();
    uploads.sort();
    still.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    Ok(format!(
        "inside copies 10 size 1920x1080 view_change_median_ms {:.2} view_change_p95_ms {:.2} placing_median_ms {:.3} placing_p95_ms {:.3} meshes_uploaded_per_view_median {} max {} still_placing_median_ms {:.3} still_placing_p95_ms {:.3} still_uploads {still_uploads}\n",
        ms(changes[VIEWS / 2]),
        ms(changes[VIEWS * 95 / 100]),
        ms(placed[VIEWS / 2]),
        ms(placed[VIEWS * 95 / 100]),
        uploads[VIEWS / 2],
        uploads[VIEWS - 1],
        ms(still[VIEWS / 2]),
        ms(still[VIEWS * 95 / 100]),
    ))
}

/// One render's wall time, its readback included.
fn time(renderer: &mut Renderer, scene: &RenderScene) -> Result<Duration, Box<dyn Error>> {
    let started = Instant::now();
    renderer.render(scene).map_err(|f| f.to_string())?;
    Ok(started.elapsed())
}

/// `copies` of the part, four to a row, 1.2 of its extent apart, each a body.
fn copies_of(mesh: &Mesh, copies: usize, extent: f64) -> Vec<RenderBody> {
    (0..copies)
        .map(|copy| {
            let shift = [
                (copy % 4) as f64 * extent * 1.2,
                (copy / 4) as f64 * extent * 1.2,
                0.0,
            ];
            let moved = |p: &[f64; 3]| [p[0] + shift[0], p[1] + shift[1], p[2] + shift[2]];
            let mut mesh = mesh.clone();
            mesh.positions = mesh.positions.iter().map(moved).collect();
            mesh.edge_points = mesh.edge_points.iter().map(moved).collect();
            RenderBody {
                body: u32::try_from(copy + 1).unwrap_or(u32::MAX),
                mesh,
                highlight_faces: vec![],
                highlight_edges: vec![],
            }
        })
        .collect()
}

fn bounds(mesh: &Mesh) -> ([f64; 3], [f64; 3]) {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in mesh.positions.iter().chain(&mesh.edge_points) {
        for k in 0..3 {
            low[k] = low[k].min(p[k]);
            high[k] = high[k].max(p[k]);
        }
    }
    (low, high)
}

/// The bodies' box's centre, and half its diagonal.
fn centre_and_reach(bodies: &[RenderBody]) -> ([f64; 3], f64) {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for body in bodies {
        let (l, h) = bounds(&body.mesh);
        for k in 0..3 {
            low[k] = low[k].min(l[k]);
            high[k] = high[k].max(h[k]);
        }
    }
    let centre = std::array::from_fn(|k| (low[k] + high[k]) / 2.0);
    let half: f64 = (0..3)
        .map(|k| (high[k] - low[k]) / 2.0)
        .map(|h| h * h)
        .sum();
    (centre, half.sqrt())
}

/// The `view`th camera of an orbit about `target`, 12 degrees apart, from above, its whole
/// reach in a 30-degree view.
fn orbit(target: [f64; 3], reach: f64, view: usize) -> Camera {
    let angle = (view as f64 * 12.0).to_radians();
    let distance = reach / libm::sin(15f64.to_radians()) * 1.1;
    let along = [libm::cos(angle), libm::sin(angle), 0.6];
    let length = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
    Camera {
        projection: Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: std::array::from_fn(|k| target[k] + along[k] / length * distance),
        target,
        up: [0.0, 0.0, 1.0],
    }
}
