//! Picking at scale (M5.7's benchmark; docs/roadmap/m5-app.md, open decision 4): the
//! renderer's frame alone, as the app's viewport draws it (`Renderer::prepare`, one draw a
//! face, each face's picking ID its instance), on the corpus's largest part, NIST's CTC 02
//! (664 faces, its 1,647 edges drawn), each frame asking to pick a 13 by 13 square (the
//! selection's edge band about the pointer, the prototype's 6 points at one pixel a point)
//! and taking the answer the frame before asked
//! for, as the app's hover does every frame the pointer moves. Each frame is timed from its
//! prepare to the GPU's end of it, the answer's take included, so a frame that picks shows
//! against the 16.7 ms of a 60 Hz display.
//!
//! Renderer-only: it waits on the GPU at the end of each frame, so each answer is ready by
//! the next. The app never waits on the GPU; its answer comes about two frames after its
//! ask, and egui's own drawing and the UI's work are outside these times.
//!
//! Five runs of each size: the view still and the pointer moving (a hover), and the view
//! orbiting under it (a hover while orbiting, which also places every face again), each
//! with nothing outlined, and with a selection of ten faces and a hovered one outlined (the
//! outline pass the app draws a selection with), and with nothing outlined but the
//! vertices picked too (M5.7: each vertex's point projected, and a disc drawn into the
//! picking layer for each near the square); the view still with the whole body
//! outlined (every face highlighted: a body's row selected in the browser); and first, as
//! the baseline, the view still with nothing outlined and nothing picked. Each line gives
//! the frames' median and 95th percentile, how many picks were answered, and how many
//! frames after its own each answer came. The part is meshed by a kernel worker at half a
//! pixel of a 1080-pixel view of it, as `views` meshes it.
//!
//! Then Select Other's cast (M5.7, `render::hits`), on the CPU alone, of ten copies of the
//! part (four to a row, 1.2 of its extent apart, as the views bench lays them), in a 1920 by
//! 1080 perspective view: the time to make the copies' boxes once (`Pickable::new`), and
//! each of 120 casts, their median, 95th percentile, and most, and how many each found. Two
//! views: `fitted`, the copies filling the view, the pointers across its middle third; and
//! `crowd`, zoomed out until each copy is about 12 points across, the pointers over the
//! copies, where every edge and vertex of a copy under the pointer is within its threshold
//! (each cast's most, its nearest 256, each cast on again for whether a face hides it). Its
//! budget, 2 ms on the UI thread (docs/design/m5.0-app.md, section 9, as built), is read
//! from these lines.
//!
//! Last, the edge batch (N5: a frame that picks draws each edge whose box lands near its
//! square again, toward the eye, so a concave edge is picked): the ten copies drawn as ten
//! bodies at 1920 by 1080, picking each frame, the view still and orbiting, with the edges
//! picked and not (`Frame::edges`), each run's frames and the CPU's placing of the faces and
//! edges (`Renderer::placed`, where every edge's box is tested against the square) by their
//! medians; the batch's cost is the difference.
//!
//! Recorded, never budgeted: it fails on no time. Run with
//! `SILICO_KERNEL_WORKER=target/release/silico-kernel-worker SILICO_BENCH_BREP=part.brep
//! pixi run cargo bench -p silico-render --bench picking`, the B-rep Silico's (as
//! `silico-brep-convert` writes it from the corpus's), after
//! `pixi run cargo build --release -p silico-kernel --bin silico-kernel-worker` (bench.yml's
//! `picking`). On lavapipe, as every render bench; `-- --gpu` draws on the machine's own
//! GPU instead (its first adapter that isn't a CPU's).

use std::error::Error;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::kernel::{Blob, KernelWorker, ShapeData};
use silico_protocol::{Camera, Mesh, Projection};
use silico_render::render::{Element, Frame, Outlines, Pickable, PixelRect, Renderer, hits};
use silico_render::worker::RenderAdapter;

/// The frames timed in each run, after the frames that upload the part's faces.
const FRAMES: usize = 120;
/// The image sizes: a laptop's viewport, and a desktop's.
const SIZES: [(u32, u32); 2] = [(1280, 720), (1920, 1080)];
/// The picking square's side: the selection's band of 6 pixels about the pointer's (6
/// points, `HIT_EDGE_POINTS`, at one pixel a point).
const SQUARE: u32 = 13;

fn main() -> Result<(), Box<dyn Error>> {
    let gpu = std::env::args().any(|arg| arg == "--gpu");
    if !gpu {
        silico_render::lavapipe::reexec_with()?;
    }
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
    let mesh = Arc::new(kernel.tessellate(&shape, extent / 1080.0 / 2.0)?);
    let (low, high) = bounds(&mesh);
    let target: [f64; 3] = std::array::from_fn(|k| (low[k] + high[k]) / 2.0);
    let reach = (0..3)
        .map(|k| (high[k] - low[k]) / 2.0)
        .map(|h| h * h)
        .sum::<f64>()
        .sqrt();
    let adapter = if gpu {
        silico_render::adapters(RenderAdapter::Any)?
            .into_iter()
            .find(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
            .ok_or("no adapter but a CPU's")?
    } else {
        silico_render::adapters(RenderAdapter::Software)?
            .into_iter()
            .next()
            .ok_or("no software rasterizer")?
    };
    let (device, queue) = silico_render::render::device(&adapter)?;
    let mut renderer = Renderer::new(
        &adapter,
        device.clone(),
        queue.clone(),
        silico_render::render::OUTPUT_FORMAT,
    )?;
    renderer.upload(1, &mesh).map_err(|f| f.to_string())?;
    let faces = u32::try_from(mesh.face_triangles.len())?;
    let mut out = String::new();
    out += &format!("adapter {}\n", adapter.get_info().name);
    out += &format!(
        "part {} faces {} triangles {} edges {} vertices\n",
        mesh.face_triangles.len(),
        mesh.triangles.len(),
        mesh.edge_point_counts.len(),
        mesh.vertex_points.len()
    );
    out += "scope renderer only: each frame waits on the GPU; the app's answer comes about 2 frames after its ask\n";
    // Ten faces selected, spread over the part's numbering, and one hovered; and every face
    // of the body.
    let selected: Vec<(u32, Element)> = (0..10)
        .map(|i| (1, Element::Face(1 + i * faces / 10)))
        .collect();
    let whole: Vec<(u32, Element)> = (1..=faces).map(|n| (1, Element::Face(n))).collect();
    let hovered = [(1, Element::Face(1 + faces / 2))];
    let mut serial = 0u64;
    for (width, height) in SIZES {
        // The still view with nothing outlined and no picking first: what the picking costs
        // is the difference.
        let runs = [
            (false, Outlined::Nothing, false, false),
            (false, Outlined::Nothing, true, false),
            (false, Outlined::Nothing, true, true),
            (false, Outlined::Ten, true, false),
            (true, Outlined::Nothing, true, false),
            (true, Outlined::Nothing, true, true),
            (true, Outlined::Ten, true, false),
            (false, Outlined::Whole, true, false),
        ];
        for (orbiting, outlined, asking, vertices) in runs {
            {
                let none: &[(u32, Element)] = &[];
                let (highlights, hover) = match outlined {
                    Outlined::Nothing => (none, none),
                    Outlined::Ten => (selected.as_slice(), hovered.as_slice()),
                    Outlined::Whole => (whole.as_slice(), none),
                };
                let frame = |r: &mut Renderer, i: usize, serial: u64| {
                    let camera = orbit(target, reach, if orbiting { i } else { 0 });
                    // The pointer across the view's middle third, a pixel a frame.
                    let at = |side: u32, step: usize| {
                        let third = side / 3;
                        let moved = u32::try_from(step).unwrap_or(0) % third.max(1);
                        i32::try_from(third + moved).unwrap_or(0) - (SQUARE / 2) as i32
                    };
                    let picking = PixelRect {
                        min: [at(width, i), at(height, i / 3)],
                        size: [SQUARE, SQUARE],
                    };
                    let started = Instant::now();
                    let commands = r
                        .prepare(&Frame {
                            serial,
                            camera: &camera,
                            size_px: [width, height],
                            pixels_per_point: 1.0,
                            bodies: &[1],
                            highlights,
                            hovered: hover,
                            outlines: Outlines::DEFAULT,
                            picking: asking.then_some(picking),
                            // The edges picked, as the selection's filter starts (N5: each
                            // near the square drawn again toward the eye).
                            edges: true,
                            // The selection's 7-point vertex discs (M5.7).
                            vertices: vertices.then_some(7.0),
                            // None, as the app's frames (its theme's VIEWPORT_FRAME_BG).
                            background: None,
                        })
                        .map_err(|f| f.to_string())?;
                    queue.submit([commands]);
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: Some(Duration::from_secs(60)),
                        })
                        .map_err(|e| e.to_string())?;
                    let answer = r.take_picking();
                    Ok::<_, String>((started.elapsed(), answer))
                };
                // Its faces uploaded at this size, and the readbacks under way.
                for i in 0..3 {
                    serial += 1;
                    frame(&mut renderer, i, serial)?;
                }
                let mut times = Vec::with_capacity(FRAMES);
                let mut answered = 0usize;
                let mut late: Vec<u64> = Vec::new();
                let mut hits = 0usize;
                for i in 0..FRAMES {
                    serial += 1;
                    let (took, answer) = frame(&mut renderer, i, serial)?;
                    times.push(took);
                    if let Some(answer) = answer {
                        answered += 1;
                        late.push(serial - answer.frame);
                        hits += usize::from(answer.hit.is_some());
                    }
                }
                times.sort();
                late.sort_unstable();
                let ms = |d: Duration| d.as_secs_f64() * 1e3;
                out += &format!(
                    "size {width}x{height} view {} outlines {} picking {} vertices {} renderer_frame_median_ms {:.2} renderer_frame_p95_ms {:.2} picks_answered {answered}/{FRAMES} hits {hits} frames_late_median {} frames_late_max {}\n",
                    if orbiting { "orbiting" } else { "still" },
                    match outlined {
                        Outlined::Nothing => "off",
                        Outlined::Ten => "ten_and_hover",
                        Outlined::Whole => "whole_body",
                    },
                    if asking { "on" } else { "off" },
                    if vertices { "on" } else { "off" },
                    ms(times[FRAMES / 2]),
                    ms(times[FRAMES * 95 / 100]),
                    late.get(late.len() / 2).copied().unwrap_or(0),
                    late.last().copied().unwrap_or(0),
                );
            }
        }
    }
    out += &cast(&mesh, extent, View::Fitted);
    out += &cast(&mesh, extent, View::Crowd);
    out += &edge_batch(&mut renderer, &device, &queue, &mesh, extent, &mut serial)?;
    std::io::stdout().write_all(out.as_bytes())?;
    Ok(())
}

/// The edge batch's cost (N5): ten copies of the part (as [`cast`] lays them), ten bodies in a
/// 1920 by 1080 perspective view of them all, each frame picking the selection's square under
/// a pointer across the view's middle third and taking the answer the frame before asked for,
/// as the hover does; the view still and orbiting, with the edges picked (each edge whose box
/// lands near the square drawn again toward the eye, `Frame::edges`) and not. Each line gives
/// the frames' median and 95th percentile, and the median of the CPU's placing of the faces
/// and edges (`Renderer::placed`), where every edge's box is tested against the square, so
/// the batch's cost is the difference between a run with the edges picked and one without.
fn edge_batch(
    renderer: &mut Renderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mesh: &Mesh,
    extent: f64,
    serial: &mut u64,
) -> Result<String, Box<dyn Error>> {
    let (width, height) = (1920u32, 1080u32);
    let copies = copies_of(mesh, 10, extent);
    // Bodies 2 to 11, after the part's own.
    let bodies: Vec<u32> = (2..=11).collect();
    for (body, copy) in bodies.iter().zip(&copies) {
        renderer.upload(*body, copy).map_err(|f| f.to_string())?;
    }
    let (low, high) = copies.iter().map(|copy| bounds(copy)).fold(
        ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
        |(lo, hi), (l, h)| {
            (
                std::array::from_fn(|k| lo[k].min(l[k])),
                std::array::from_fn(|k| hi[k].max(h[k])),
            )
        },
    );
    let target: [f64; 3] = std::array::from_fn(|k| (low[k] + high[k]) / 2.0);
    let reach = (0..3)
        .map(|k| (high[k] - low[k]) / 2.0)
        .map(|h| h * h)
        .sum::<f64>()
        .sqrt();
    let edges_drawn: usize = copies.iter().map(|c| c.edge_point_counts.len()).sum();
    let mut out = String::new();
    for (orbiting, edges) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut frame = |i: usize, serial: u64| {
            let camera = orbit(target, reach, if orbiting { i } else { 0 });
            let at = |side: u32, step: usize| {
                let third = side / 3;
                let moved = u32::try_from(step).unwrap_or(0) % third.max(1);
                i32::try_from(third + moved).unwrap_or(0) - (SQUARE / 2) as i32
            };
            let picking = PixelRect {
                min: [at(width, i), at(height, i / 3)],
                size: [SQUARE, SQUARE],
            };
            let started = Instant::now();
            let commands = renderer
                .prepare(&Frame {
                    serial,
                    camera: &camera,
                    size_px: [width, height],
                    pixels_per_point: 1.0,
                    bodies: &bodies,
                    highlights: &[],
                    hovered: &[],
                    outlines: Outlines::DEFAULT,
                    picking: Some(picking),
                    edges,
                    vertices: None,
                    background: Some([0x00, 0x00, 0x00]),
                })
                .map_err(|f| f.to_string())?;
            let placed = renderer.placed();
            queue.submit([commands]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(60)),
                })
                .map_err(|e| e.to_string())?;
            let answer = renderer.take_picking();
            Ok::<_, String>((started.elapsed(), placed, answer))
        };
        // The copies' faces uploaded, and the readbacks under way.
        for i in 0..3 {
            *serial += 1;
            frame(i, *serial)?;
        }
        let mut times = Vec::with_capacity(FRAMES);
        let mut placing = Vec::with_capacity(FRAMES);
        let (mut answered, mut on_edges) = (0usize, 0usize);
        for i in 0..FRAMES {
            *serial += 1;
            let (took, placed, answer) = frame(i, *serial)?;
            times.push(took);
            placing.push(placed);
            if let Some(answer) = answer {
                answered += 1;
                on_edges += usize::from(answer.edge.is_some());
            }
        }
        times.sort();
        placing.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        out += &format!(
            "edge_batch copies 10 edges {edges_drawn} size {width}x{height} view {} edges_picked {} renderer_frame_median_ms {:.2} renderer_frame_p95_ms {:.2} placed_median_ms {:.3} picks_answered {answered}/{FRAMES} picks_with_an_edge {on_edges}\n",
            if orbiting { "orbiting" } else { "still" },
            if edges { "on" } else { "off" },
            ms(times[FRAMES / 2]),
            ms(times[FRAMES * 95 / 100]),
            ms(placing[FRAMES / 2]),
        );
    }
    Ok(out)
}

/// Where Select Other's casts are made from: the copies filling the view, the pointers across
/// its middle third; or zoomed out until each copy (`extent` across) is 12 pixels across, the
/// pointers over them.
#[derive(Clone, Copy)]
enum View {
    Fitted,
    Crowd,
}

/// The casts Select Other makes, on ten copies of `mesh`, from `view`: their boxes' time, and
/// the casts' times and counts, as one line.
fn cast(mesh: &Mesh, extent: f64, view: View) -> String {
    const CASTS: usize = 120;
    let (width, height) = (1920u32, 1080u32);
    let copies = copies_of(mesh, 10, extent);
    let started = Instant::now();
    let pickables: Vec<Pickable> = copies.into_iter().map(Pickable::new).collect();
    let boxes = started.elapsed();
    let bodies: Vec<(u32, &Pickable)> = (1u32..).zip(&pickables).collect();
    let (low, high) = pickables.iter().map(|p| bounds(p.mesh())).fold(
        ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
        |(lo, hi), (l, h)| {
            (
                std::array::from_fn(|k| lo[k].min(l[k])),
                std::array::from_fn(|k| hi[k].max(h[k])),
            )
        },
    );
    let target: [f64; 3] = std::array::from_fn(|k| (low[k] + high[k]) / 2.0);
    let reach = (0..3)
        .map(|k| (high[k] - low[k]) / 2.0)
        .map(|h| h * h)
        .sum::<f64>()
        .sqrt();
    let fitted = orbit(target, reach, 0);
    let (camera, name, over) = match view {
        View::Fitted => {
            let third = |side: u32| (f64::from(side) / 3.0, f64::from(side) * 2.0 / 3.0);
            (fitted, "fitted", [third(width), third(height)])
        }
        View::Crowd => {
            // Back along the orbit's line until a copy's extent is 12 pixels of the view's
            // 1080 (30 degrees high): 2 tan 15° of the distance is the view's height there.
            let away = extent * f64::from(height) / 12.0 / (2.0 * libm::tan(15f64.to_radians()));
            let along: [f64; 3] = std::array::from_fn(|k| fitted.eye[k] - target[k]);
            let length = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
            let camera = Camera {
                eye: std::array::from_fn(|k| target[k] + along[k] / length * away),
                ..fitted
            };
            // The pointers over the copies: the box their corners land in.
            let corners = silico_protocol::box_corners((low, high));
            let landed: Vec<[f64; 2]> = corners
                .iter()
                .filter_map(|corner| {
                    silico_render::render::project(&camera, [width, height], *corner)
                })
                .collect();
            let span = |k: usize| {
                landed
                    .iter()
                    .map(|p| p[k])
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), c| {
                        (lo.min(c), hi.max(c))
                    })
            };
            (camera, "crowd", [span(0), span(1)])
        }
    };
    let mut times = Vec::with_capacity(CASTS);
    let mut found = Vec::with_capacity(CASTS);
    for i in 0..CASTS {
        // A 12 by 10 grid over the pointers' span.
        let (col, row) = ((i % 12) as f64, (i / 12) as f64);
        let [(x0, x1), (y0, y1)] = over;
        let pointer = [
            (x0 + (x1 - x0) * col / 11.0).clamp(0.0, f64::from(width) - 1.0),
            (y0 + (y1 - y0) * row / 9.0).clamp(0.0, f64::from(height) - 1.0),
        ];
        let started = Instant::now();
        let under = hits(&camera, [width, height], 1.0, &bodies, pointer);
        times.push(started.elapsed());
        found.push(under.len());
    }
    times.sort();
    found.sort_unstable();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    format!(
        "cast {name} copies 10 size {width}x{height} boxes_ms {:.2} cast_median_ms {:.3} cast_p95_ms {:.3} cast_max_ms {:.3} hits_median {} hits_max {}\n",
        ms(boxes),
        ms(times[CASTS / 2]),
        ms(times[CASTS * 95 / 100]),
        ms(times[CASTS - 1]),
        found[CASTS / 2],
        found[CASTS - 1],
    )
}

/// `copies` of the part, four to a row, 1.2 of its extent apart (the views bench's layout),
/// each moved whole: its positions, edge points, and vertex points.
fn copies_of(mesh: &Mesh, copies: usize, extent: f64) -> Vec<Arc<Mesh>> {
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
            mesh.vertex_points = mesh.vertex_points.iter().map(moved).collect();
            Arc::new(mesh)
        })
        .collect()
}

/// What a run outlines: nothing; ten faces selected and one hovered; or every face of the
/// body (its browser row selected).
#[derive(Clone, Copy)]
enum Outlined {
    Nothing,
    Ten,
    Whole,
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

/// The `view`th camera of an orbit about `target`, 3 degrees apart, from above, its whole
/// reach in a 30-degree view.
fn orbit(target: [f64; 3], reach: f64, view: usize) -> Camera {
    let angle = (view as f64 * 3.0).to_radians();
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
