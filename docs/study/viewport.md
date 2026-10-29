# The viewport on `re_renderer` 0.38.1

A study note for M5's design (M5.0) and its viewport steps (M5.6, M5.7). It covers four things, each read from the code:

- how Rerun's viewer embeds `re_renderer` in egui, the way the app will;
- its outline and picking passes;
- what crisp edges, silhouettes, and hidden-line display need beyond what M4.6 drew;
- what giving each B-rep face a picking ID of its own costs, measured on the corpus's largest part.

It describes what the code does and what was measured. The decisions stay with M5.0.

**Sources.**

- Rerun's repository at tag `0.38.1` (commit `b08c599e934b0dedee1e95fd74a989a1582ce3d5`). Paths below are relative to its root.
- The published `re_renderer` 0.38.1 crate. Its files are cited as `re_renderer/src/...` and `re_renderer/shader/...`; in Rerun's tree they sit under `crates/viewer_support/re_renderer/`.
- Silico's `silico-render` at `main` (`837347ac`).
- The measurements are from a scratch program outside the repository, described under *Method*. They were taken on 2026-10-27 on an Apple M4 Pro, on Metal, on one thread.

## In brief

- **Embedding is a paint callback.**
  - Rerun builds one `ViewBuilder` per view per frame and hands it to egui as an `egui_wgpu` paint callback.
  - The callback draws it in `prepare` and composites it into egui's render pass in `paint`.
  - The `RenderContext` lives in egui-wgpu's callback resources and shares eframe's device and queue.
  - eframe itself runs without depth or MSAA, because each view does its own.
- **Picking is per instance, and a draw is per mesh.**
  - `re_renderer` batches instances that share a `GpuMesh` into one instanced draw.
  - `silico-render` makes each B-rep face a `GpuMesh` of its own, so that each face gets an ID. That makes each face its own batch, drawn once in each pass it takes part in.
- **What that costs.** Every frame here waited for the GPU, so a frame's time is the CPU's and the GPU's added together; a viewport that pipelines them would take about the larger of the two, which wasn't measured.
  - **NIST CTC 02 (664 faces), the corpus's largest part.** The per-face meshes take 2.0 ms a frame against 1.0 ms for one mesh. That is well inside a frame, so it passes the M5 plan's open decision 4, which keeps per-instance picking unless the largest part misses a frame. A camera move that rebuilds every mesh adds 3.1 ms.
  - **Ten copies of it (6,640 faces).** The per-face meshes take 14.6 ms of CPU time before submitting: inside 60 Hz (16.7 ms), past 120 Hz (8.3 ms). With the wait, 28 ms in all, against 1.4 ms for one mesh. A camera move that rebuilds every mesh adds 33 ms.
- **The way out needs no fork.**
  - `re_renderer`'s `Renderer` and `DrawData` traits are public, and Rerun ships an example of a renderer outside the crate.
  - A 200-line renderer in the scratch program draws a body in one call and writes each pixel's face number into the picking layer.
  - It costs what one mesh costs, and its picking IDs matched the per-face meshes' on every pixel of the picking rectangle.
  - Caveats: it ran on an M4 Pro's Metal only, not on lavapipe, where agents' renders draw. Its colour wasn't compared with `silico-render`'s. And it lights per pixel where M4.6 lights per vertex, so curved faces would shade slightly differently.
  - The same renderer lights with M4.6's lights, fixed to the camera, in its shader. That removes one reason every orbit rebuilds every mesh, as the next point shows.
- **Light and position are baked for one view today.**
  - `re_renderer`'s mesh shader lights from two lights fixed in the world, so `silico-render` bakes M4.6's camera-fixed lights into vertex colours.
  - It also places every vertex relative to the eye (or the target), in f64, before it's f32: M4.6's precision design.
  - In a viewport, either one means rebuilding every face's mesh whenever the camera moves: 3.1 ms for CTC 02 and 33 ms for 6,640 faces (section 5).
- **Edges, silhouettes, and hidden lines.**
  - Crisp B-rep edges exist: lines of a width in points, offset in depth. They're blended with a mode Rerun documents as suited only to 2D.
  - Silhouettes of curved faces, and hidden or dashed lines, exist nowhere in `re_renderer`.
  - Each needs Silico's own drawing, which the same public traits allow.

## 1. How Rerun's viewer embeds `re_renderer` in egui

**The device.**

- At startup the viewer builds one `RenderContext` from eframe's wgpu state: its adapter, device, queue, and target format. It inserts the context into egui-wgpu's `callback_resources` (`crates/top/re_viewer/src/lib.rs:268-282`).
- eframe's adapter and device are chosen with `re_renderer`'s `device_caps` (`crates/top/re_viewer/src/lib.rs:226-241`).
- eframe runs with `depth_buffer: 0` and `multisampling: 0`, commented "the 3D views do their own MSAA" (`crates/top/re_viewer/src/native.rs:52-53`).

**The frame.**

- `begin_frame` runs inside the UI closure. Its comment says it "may actually be called multiple times per egui frame, if we have a multi-pass layout frame" (`crates/top/re_viewer/src/app/ui.rs:79-83`).
- `before_submit` runs after every view is built (`crates/top/re_viewer/src/app/ui.rs:158`).
- `before_submit` submits its own encoder, separate from eframe's submit (`re_renderer/src/context.rs:403-427`).

**A 3D view, each frame** (`crates/views/re_view_spatial/src/ui_3d.rs`):

1. It takes its size in pixels from the UI rectangle and `pixels_per_point`, and returns early at size zero (205-209).
2. It fills a `TargetConfiguration` with `pixels_per_point`, and with an outline configuration only when something is outlined (263-285).
3. It makes a `ViewBuilder` and queues each visualizer's draw data (287, 401-414).
4. It adds the callback to egui's painter (423-427).

A resize needs nothing special: the view builder is new each frame at the current size. Its targets come from a texture pool, which frees textures unused for a frame (`re_renderer/src/wgpu_resources/texture_pool.rs:123`).

**The callback** (`crates/viewer_support/re_viewer_context/src/gpu_bridge/re_renderer_callback.rs`):

- `ReRendererCallback` implements `egui_wgpu::CallbackTrait`.
- `prepare` takes the `RenderContext` from the callback resources and calls `view_builder.draw`, which returns its command buffer to egui-wgpu (28-51).
- `paint` sets the viewport to the rectangle in physical pixels, rounded, so the view's texture isn't stretched (53-87). It then calls `composite`, which draws the view's colour into egui's pass (89).

**What `draw` does** (`re_renderer/src/view_builder.rs:732-901`), in order:

1. The main pass, into a 4× multisampled target, resolved.
2. The picking pass, if one was asked for.
3. The outline mask and its passes.
4. A screenshot, if one was asked for.

`composite` runs only the compositing phase (929-940).

**Formats.**

- The main colour target is `Rgba8UnormSrgb` (`view_builder.rs:331`).
- Depth is `Depth32Float`, reverse-z, compared with `GreaterEqual` (385, 409-426).
- MSAA is 4× or off (`re_renderer/src/context.rs:31-49`).

**For Silico's app.** This is what M5.6 builds, with the `RenderContext` in eframe's callback resources. Two consequences:

- A `silico-render` that draws for both the worker's renders and the app's viewport needs its drawing split from its device setup. Today `Renderer::new` makes its own device (`crates/silico-render/src/render.rs:96-110`); in the app the device is eframe's.
- The UI thread does the drawing, so everything per frame has to be cheap: meshes uploaded once a model, never per frame (M5.6's own test). That is what sections 4 and 5 measure.

## 2. Outlines

- **Method.** Jump flooding, after Ben Golus's method (`re_renderer/src/draw_phases/outlines.rs:11-44`).
  - The outlined objects are drawn into a mask of two 8-bit channels, A and B (`Rg8Uint`). The mask has a depth buffer of its own, so an outline shows even where something is in front (152, 244-256).
  - The mask is 4× multisampled where the device can sample multisampled textures (205-214).
  - An initialising pass and a series of step passes, each full screen, find each pixel's nearest masked pixel (382-434). The compositor then draws pixels within the radius (`re_renderer/shader/composite.wgsl:52-60`).
  - The number of steps is `log2` of the radius rounded up to a power of two, plus one (`outlines.rs:525-527`). So M4.6's 1.5-pixel outline takes two step passes.
- **Per instance.** An `OutlineMaskPreference` names a value in channel A, in channel B, or both: up to 255 groups each (`outlines.rs:58-66`).
- **How Rerun uses it.**
  - Hover is channel A and selection channel B, each selected item a number of its own (`crates/viewer_support/re_view/src/view_highlights.rs:25-30, 91-97`).
  - The colours and width come from egui's hover and selection strokes (`crates/viewer_support/re_view/src/outlines.rs:13-27`).
  - So two kinds of highlight can show at once. That is what SolidWorks' pre-selection and selection need (`docs/ux/solidworks.md`, Selection).
- **Mesh outlines are per instance.** One outlined face therefore needs a mesh or an instance of its own, or a renderer of Silico's own that masks by face (section 5).

## 3. Picking

- **The target.**
  - `Rgba32Uint`: red and green hold a 64-bit object ID, blue and alpha a 64-bit instance ID. Its depth is `Depth32Float`, read back too. No MSAA, so each pixel is one ID (`re_renderer/src/draw_phases/picking_layer.rs:6-8, 89-128, 159-168`).
  - Only the rectangle asked for is drawn, through a cropped projection (229-254). A rectangle partly or wholly off screen is allowed; what's off it reads as if the target were bigger (`view_builder.rs:298-301`).
- **How Rerun asks for it** (`crates/views/re_view_spatial/src/picking_ui.rs:44-63`):
  - nothing is picked while a drag is under way;
  - the rectangle is centred on the pointer, twice the interaction radius (5 points) times `pixels_per_point`, from 8 to 128 pixels;
  - it is larger than needed, "so we can use it to counter-act delays".
- **The result comes later.**
  - The copy is recorded in `draw`, and mapped at the next `begin_frame`.
  - `PickingLayerProcessor::readback_result` returns the newest result ready (`picking_layer.rs:395-407`).
  - A result not collected within a frame is dropped (`re_renderer/src/allocator/gpu_readback_belt.rs:351-355`).
  - Rerun keeps the last hit while no new one arrives, noting "occasional frames with 0 and then with 2 picking results" (`crates/views/re_view_spatial/src/picking.rs:246-252`). A hover in Silico's viewport is therefore at least a frame behind the pointer, as Rerun's is.
- **From pixel to item.**
  - Rerun takes the non-zero pixel nearest the pointer, and the world position from its depth.
  - It maps the ID to its own instance path, ordering hits from overlays, then the GPU, then textured rectangles (`crates/views/re_view_spatial/src/picking.rs:176-245`; `crates/viewer_support/re_view/src/instance_hash_conversions.rs:14-24`).
  - Meshes, lines, and points share the one ID buffer. A line writes its ID only where its coverage is at least half (`re_renderer/shader/lines.wgsl:380-386`).
- **Granularity.**
  - A mesh instance has one ID. The ID is an instance-stepped vertex attribute (`re_renderer/src/renderer/mesh_renderer.rs:52, 77-79`), passed to the picking fragment shader unchanged (`re_renderer/shader/instanced_mesh.wgsl:68, 105-106`).
  - Nothing per vertex or per triangle exists, and a material carries no ID.
  - A line batch has one object ID, and each strip an instance ID (`re_renderer/src/line_drawable_builder.rs:241, 502-540`). That is how `silico-render` names each edge (`crates/silico-render/src/render.rs:409-441`).
  - A point has one ID per point.
- **Batching.** M4.6's design (line 25) says each face is "a mesh and a draw of its own". That holds for how `silico-render` builds faces. `re_renderer` itself batches by mesh:
  - Instances are grouped by their `GpuMesh`'s pointer and cull mode (`mesh_renderer.rs:176-182, 255-265`).
  - Each group is one instanced `draw_indexed` per material per phase (735-782).
  - Each group is drawn in the opaque phase, the picking phase, and the outline phase when any of its instances is outlined (383-399, 797-818).
  - So instances of one mesh would share a draw. `silico-render` gives each face its own `GpuMesh` (`crates/silico-render/src/render.rs:366-385`), so each face is its own batch: two or three draws a frame.

## 4. Edges, silhouettes, hidden lines, and light: what M4.6 drew, and what the viewport needs

**What M4.6 draws** (`docs/design/m4.6-render.md`, Look; `crates/silico-render/src/render.rs`):

- every B-rep edge, as the display mesh's polylines;
- edges 2 points wide, dark, alpha-blended, at depth offset 1 (`EDGE_DEPTH_OFFSET`, 409-441);
- faces lit by M4.6's camera-fixed lights, baked into vertex colours with the normals set to zero, so `re_renderer` draws them unlit (309-407);
- highlights as a half-strength blue tint and a 1.5-pixel outline.

Hidden edges aren't drawn, and nothing draws a silhouette.

**Crisp edges.** What exists:

- **Width.** A line's width is in scene units when positive, in UI points when negative (`re_renderer/src/size.rs:3-9`), so a 2-point edge stays 2 points at any zoom. A line's points are limited to the square of the device's largest 2D texture side (`re_renderer/src/renderer/lines.rs:414-416`).
- **Antialiasing.** By default lines antialias by coverage, written as alpha-to-coverage under MSAA (`lines.wgsl:331-347`; `lines.rs:728`). `enable_alpha_blending` switches to blending without depth writes. Its own documentation says it "is for now only suited for 2D views, since it doesn't handle depth ordering within draw calls correctly yet" (`re_renderer/src/line_drawable_builder.rs:70-76`).
  - `silico-render` uses blending (`render.rs:147`), because alpha-to-coverage gave a 1.5-pixel line about half its coverage on lavapipe (M4.6, Look).
  - In a viewport, where edges cross each other at depth, the blended mode's ordering caveat applies. The coverage mode on a hardware GPU's MSAA is the other choice. Neither was compared on screen here.
- **Depth offset.** `DepthOffset` is an `i16` per line batch. It is applied in the vertex shader by scaling `w` by `1 − ε·offset`, not by the pipeline's depth bias (`re_renderer/src/depth_offset.rs:1-16`; `re_renderer/shader/utils/depth_offset.wgsl:73-97`).
  - It applies to lines, points, rectangles, and voxels. The mesh shader doesn't apply it (`instanced_mesh.wgsl` has no `depth_offset` import).
  - Every pipeline's depth bias is zero (`view_builder.rs:421-425`).
  - Lines are sorted to draw last (`lines.rs:218`), and `GreaterEqual` lets a later draw win at equal depth.
  - The file's own notes say the offset's precision depends on the near plane, and that "a higher near plane distance" or "a larger depth offset" are the only reliable remedies (`depth_offset.wgsl`, the comment above `apply_depth_offset`).
  - M4.6 sets the near plane at a millionth of the farthest point's distance (`render.rs:50`). Whether offset 1 is enough at that near plane for every zoom is untested here.
- **Flags.** Caps at either end; a radial colour falloff (`STRIP_FLAG_COLOR_GRADIENT`); forcing orthographic spanning (`lines.rs:225-268`).

**Silhouettes.**

- A curved face's outline as seen (a cylinder's side, a fillet seen edge-on) is no B-rep edge, so M4.6 draws nothing there. Fusion and SolidWorks draw it.
- A search of `re_renderer/src` and `re_renderer/shader` for "silhouette", "hidden" and "wireframe" finds nothing.
- The outline pass outlines whatever is masked. Masking a whole body therefore outlines its outer contour only, not a boss's silhouette inside it, and in the highlight's colour.
- So silhouettes are Silico's to draw. Each of these is a possibility, none of them built or measured here:
  - edge detection in screen space, over the depth and normals;
  - silhouette edges found from the mesh's adjacency for each view;
  - exact silhouettes from OCCT's hidden-line removal in the kernel worker.
- Section 5's custom renderer shows that drawing of Silico's own fits in `re_renderer`'s phases. A pass that reads the frame's depth buffer is not something this study found an extension point for.

**Hidden-line display.**

- Fusion's visual styles include *Shaded with Hidden Edges* and *Wireframe with Hidden Edges* (`docs/ux/fusion.md:68`).
- `re_renderer`'s lines can't be dashed. A `stippling` field exists in the line data and is unpacked in the shader, but a TODO says it isn't done, and the builder doesn't expose it (`lines.rs:110-113, 135, 146`; `lines.wgsl:89, 110`).
- Every line pipeline tests depth against the main buffer (`lines.rs:733`). So hidden edges, drawn faint or dashed behind the faces, need a line renderer of Silico's own, with the depth test inverted or off.

**Light.**

- `re_renderer`'s mesh shader lights a mesh with a non-zero normal by `simple_lighting`: an ambient of 0.2 and two lights fixed in world space (`re_renderer/shader/utils/lighting.wgsl:1-9`; `instanced_mesh.wgsl:93-99`). Its TODO (#1800) asks for scene lighting.
- M4.6's look lights from the camera (Fusion's). So `silico-render` computes the light per vertex on the CPU, for one view, and stores it as the vertex colour.
- That is right for one render. In a viewport it means every face's mesh is rebuilt on every camera move. Measured in section 5: 3.1 ms for CTC 02, and 33 ms for 6,640 faces, building the meshes alone.
- A renderer of Silico's own computes the same light in its fragment shader from the view matrix (section 5), so the light needs no rebuild.
- The light isn't the only thing baked for one view. `silico-render` also places each vertex relative to the eye, or to the target for an orthographic view, subtracting in f64 before it's f32 (`render.rs:288-291`, the reference set at 260 and 278). That is M4.6's precision design: "a part 5·10⁴ mm from the origin would round there to 4e-3 mm, more than a pixel" (`docs/design/m4.6-render.md`, *Drawn about the target, or the eye*). So a camera move rebuilds the positions too.
  - A viewport that rebuilds nothing on a camera move needs that offset moved out of the vertices: positions about each body's own origin, and the body's origin minus the eye, in f64, sent as an f32 transform each frame. Or the precision traded.
  - The probe in section 5 did neither: it placed positions about the part's centre once.
- Another way, with `re_renderer`'s own meshes: keep the camera fixed in the world and move the bodies through each instance's `world_from_mesh`, which `MeshDrawData::new` takes afresh every frame anyway. `re_renderer`'s world-fixed lights then follow the camera, the eye offset becomes the transform, and no mesh is rebuilt. The light is then `re_renderer`'s formula (an ambient of 0.2 and two lights), not M4.6's, which changes M4.6's look. This wasn't tried.

## 5. The cost of a picking ID per face, measured

**What was measured.**

- **The part.** NIST's CTC 02 (`nist_ctc_02_asme1_rc`), the corpus's largest by faces: 664 faces and 1,647 edges. Counts are of the `Fa` and `Ed` shapes in the cached OCCT B-reps of the 33 parts (`~/.cache/silico/corpus/nist/brep`). It was meshed by the kernel worker at deflection 0.398, the part's extent (860) over 2 × 1,080, giving 24,460 triangles.
  - `render` meshes at half a pixel at the part's nearest depth, rounded to a power of two on M4.6's ladder (`crates/silico-core/src/command/render.rs:724-731`, `mesh_level`). So the viewport's triangle count may differ from this by up to a step of the ladder.
- **The frame.**
  - 1920 × 1080, 4× MSAA, perspective;
  - a 32 × 32 picking rectangle at the centre, read back every frame;
  - one face outlined (the whole body, for the one-mesh variant);
  - composited into an egui-sized target, as a viewport frame is;
  - faces only: no edges were drawn, though M4.6 draws every B-rep edge as a line strip (1,647 for CTC 02). So a viewport frame with edges costs more than these. The comparison between the variants holds.
- **Timing.** Median and 95th percentile of 300 frames after 30 to warm up: the CPU's time to submit, and the time until the GPU is done. Each frame waits for the GPU before the next begins, so "GPU done" is the CPU's time and the GPU's added together. A viewport that lets them overlap would take about the larger of the two; that wasn't measured.
- **The three variants:**
  - **per face:** a `GpuMesh` per face, as `silico-render` builds them;
  - **one mesh:** the same triangles as one `GpuMesh`, one ID for the body;
  - **face renderer:** a renderer of Silico's own, described below.
- **The scenes.** One copy of the part, and ten side by side (6,640 faces), for the "thousands of draws a frame" M4.6's design foresaw.

**Results.** Each figure is the median over three runs; the 95th percentiles were within 0.5 ms of the medians for one copy, and within 4 ms for ten.

| Scene | Variant | CPU to submit | GPU done | A camera move rebuilds |
|---|---|---|---|---|
| CTC 02, 664 faces | per face | 0.9 ms | 2.0 ms | every mesh: 3.1 ms |
| | one mesh | 0.11 ms | 1.0 ms | the mesh: 2.7 ms |
| | face renderer | 0.11 ms | 1.0 ms | nothing (buffers built once: 1.5 ms) |
| 10 × CTC 02, 6,640 faces | per face | 14.6 ms | 28 ms | every mesh: 33 ms |
| | one mesh | 0.26 ms | 1.4 ms | the meshes: 1.1 ms |
| | face renderer | 0.25 ms | 1.3 ms | nothing (buffers built once: 12 ms) |

Notes on the results:

- The very first run gave 5.0 ms to GPU done for one copy per face, and later runs gave 2.0 ms. The first run is left out of the table.
- The one-mesh rebuild took 1.1 ms after the per-face rebuild had run and 2.7 to 3.3 ms in the one-copy runs. It depends on the state of `re_renderer`'s upload belt, which this study didn't look into.
- With ten copies, the per-face frame's CPU time (14.6 ms) is inside 60 Hz (16.7 ms) and past 120 Hz (8.3 ms). A camera move on top of it (33 ms to rebuild) misses a 60 Hz frame.
- The rebuild times are for building the GPU meshes only, from a fixed colour. Computing each vertex's light, as `silico-render` does, adds to them.

**The face renderer.** It shows that a picking ID per face needs no fork. It is about 200 lines of Rust and 60 of WGSL in the scratch program, built against the published crate, modelled on Rerun's `examples/rust/custom_visualizer/src/height_field_renderer.rs`:

- **Data.** Each body is three vertex buffers (position, normal, the face's number), an index buffer, and a uniform with the body's object ID. Each face has its own vertices, as `silico-render`'s already do (`render.rs:330-361`), so a face's number is a flat vertex attribute.
- **Pipelines.** One each for the opaque, picking, and outline-mask phases, from `re_renderer`'s public descriptors (`PickingLayerProcessor::PICKING_LAYER_FORMAT`, `OutlineMaskProcessor::MASK_FORMAT`, and their depth and MSAA states).
  - The picking fragment shader returns the body's ID and the face's number.
  - The outline fragment shader masks one face by its number.
  - The colour fragment shader lights by M4.6's formula, from the normal in view space.
- **Registration.** The renderer is registered with `ctx.renderers_mut().register::<FaceRenderer>()` (`re_renderer/src/renderers.rs:72`; `re_renderer/src/context.rs:440`), as Rerun's example does (`examples/rust/custom_visualizer/src/main.rs:74`). Its draw data implements `DrawData` (`re_renderer/src/renderer/mod.rs:127`), and the renderer implements `Renderer` (`mod.rs:170`).
- **The shader's one catch.** A build of the published crate reads shaders from an in-memory file system, not from disk (`re_renderer/src/file_system.rs:37-45`; `re_renderer/build.rs:134-138`). Rerun's own example builds inside Rerun's workspace, where debug builds read from disk.
  - Outside it, the shader is put into the in-memory file system first, with `get_filesystem().create_file("shader/silico_face.wgsl", ...)` (`file_system.rs:26-32, 149-158`). It's then named by that path in a `ShaderModuleDesc`.
  - Its `#import <types.wgsl>` and `<global_bindings.wgsl>` resolve beside it (`re_renderer/src/file_resolver.rs`, `resolve_clause_path`, relative to the importing file).
  - This was done in a release build. It compiled and drew.
- **Correctness.** In both scenes, the face renderer's picking rectangle named the same object and face as the per-face meshes' on all 1,024 of its 1,024 pixels, with 5 and 15 different faces in it. Its colour output wasn't compared with `silico-render`'s by pixel, and it ran on Metal only, not on lavapipe.

**What this changes.**

- **Against open decision 4.** The M5 plan keeps per-instance picking unless the corpus's largest part misses a frame in M5.7's benchmark. CTC 02 doesn't miss one per instance: 2.0 ms, or about 5 ms on a frame that also rebuilds for a camera move. So the case for a renderer of Silico's own isn't CTC 02's frame time. It rests on:
  - parts of several thousand faces (the ten-copy scene: 14.6 ms of CPU a frame, and 33 ms to rebuild for a camera move);
  - the camera-fixed light and the eye offset, which otherwise rebuild every mesh on every orbit, whatever the part;
  - an outline by face.
- M4.6's design names two ways to meet the per-face cost: a fork of `re_renderer`, or a per-triangle `R32Uint` target of Silico's own. ARCHITECTURE.md (line 205) allows a fork "only if CAD features need changes to its internals".
- The public traits give a third way that changes no internals: a renderer of Silico's own inside `re_renderer`'s phases. It draws its colour, its picking IDs, and its outline mask in the same passes as everything else.
- The same renderer is the natural home for the camera-fixed light, for an outline by face, and later for a hidden-line pass. To rebuild nothing on a camera move, it also needs the eye offset as a per-body transform (section 4, Light), which the probe didn't build. Agents' renders and the viewport could both draw through it, keeping M4.6's R1 (one renderer).
- Two costs come with it:
  - Silico owns a WGSL shader and its pipelines, built against `re_renderer`'s descriptors, which a version bump can change;
  - its shader must be registered in the in-memory file system before the renderer is created.

## For M5.0

What the design has to settle, from the above. Each item is a question, not a decision:

1. **Faces: a renderer of Silico's own, or `re_renderer`'s meshes** (an amendment to open decision 4, if taken). By decision 4's own test, CTC 02 stays per-instance: it doesn't miss a frame. The case for Silico's own is the larger parts, the rebuild on every camera move (light and eye offset), and outlines by face (section 5). The other way to avoid the rebuild is to move the bodies, not the camera, at the cost of `re_renderer`'s light formula (section 4). M5.6 and M5.7 both depend on the answer.
2. **Edge antialiasing in the viewport.** Blended, as M4.6 draws, with its documented depth-ordering caveat; or coverage under MSAA on a hardware GPU.
3. **Silhouettes.** Which of the three ways in section 4, and in which step (M5.6 promises "silhouettes").
4. **Hidden-line display.** Whether M5 has it at all (Fusion's default style shows visible edges only, M4.6, Look). If it does, it needs a line renderer of Silico's own.
5. **Hover latency.** Picking results arrive a frame or more late, so pre-selection follows the pointer a frame behind, as Rerun's does. Rerun's remedy is a larger picking rectangle.
6. **One device.** `silico-render`'s drawing separated from its device, so the worker and the app each supply theirs.

## Method

The scratch programs are outside the repository. Their sources are in a comment on the PR that added this note (#223), so the numbers can be reproduced; the programs themselves are removed once it merges.

- **The program.** A crate depending on `silico-core` (by path, at `main`), `re_renderer` 0.38.1 (`default-features = false`), and `wgpu` 30 with Metal only, built in release.
- **The part.** It loads CTC 02's cached OCCT B-rep, converted to Silico's with `silico-brep-convert`, and meshes it on a kernel worker built from `main`, at deflection 1 and then at half a pixel.
- **The frames.** It draws offscreen and composites into a `Bgra8Unorm` target, as egui's would be. It polls the device until each frame is done, so "GPU done" includes the CPU's share. Picking results still arrive a frame late, because the readback is mapped at the next `begin_frame` (`re_renderer/src/context.rs:327-329`): 329 results in 330 frames. A viewport that doesn't wait sees them that frame or later (section 3).
- **The checks.** Each variant ran twice in each run, in the order per face, one mesh, face renderer, and the whole program ran three times. The picking comparison is between the per-face and face-renderer variants' last results.
- **The rest.** Rerun's sources were read at the tag, and every citation above was checked against them. Rerun's custom-renderer example was read, not built.
