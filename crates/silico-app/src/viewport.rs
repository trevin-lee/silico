//! The viewport (docs/design/m5.0-app.md, sections 2, 3, 8, and 10; M5.6): the canvas that
//! draws the tab's published model through a renderer ([`crate::renderer`]), under the tab's
//! own camera (decision 29), with Fusion's navigation (the wheel, the middle button, F6),
//! its ViewCube at the canvas's top right, and its navigation bar at its bottom centre.
//!
//! Each tab holds its meshes ([`View`]): the newest `meshes` answer's bodies by number, each
//! with its key, its level, its box, the half pixels it serves, its mesh, and its faces' and
//! edges' names (what picking reads, M5.7). It keeps at most one `meshes` read in flight,
//! of the published model, listing every mesh it holds (`held`), and reads again only when
//! the published revision changes, or when the view settles with a body's half pixel
//! outside its `serves`, computed by the core's own rule (`views::half_pixel_at` at
//! `views::meshing_depth`), so no rule is copied here (section 7, `meshes`). The renderer
//! is told of a body once for each key and level it's meshed at, and of a body gone
//! ([`Drawing`]); each frame it's given the camera, and places the bodies itself (section
//! 10). Nothing here waits: the read is polled with `try_take` (invariant 4).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use eframe::egui::{
    self, Align, Layout, PointerButton, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, WidgetInfo,
    WidgetType,
};
use silico_core::command::meshes::{Held, MeshElement, MeshedBody, Meshes, MeshesRead};
use silico_core::command::{ModelChoice, Response, Revision, Running, SessionId, Sessions};
use silico_core::document::FeatureId;
use silico_core::kernel::Clients;
use silico_core::recompute::Key;
use silico_core::views::{NamedView, half_pixel_at, meshing_depth};
use silico_protocol::{Camera, MAX_RENDER_SIDE, Mesh, Projection};

use crate::navigate::{self, HOME, NO_BOX};
use crate::renderer::{Change, Element, FrameData, Picking, PixelRect, Present};
use crate::tab::Tab;

/// A body whose mesh the tab holds, as the newest `meshes` answer gave it.
#[derive(Debug, Clone)]
pub struct ViewBody {
    pub feature: FeatureId,
    /// Its step's key, and the level and deflection it was meshed at.
    pub key: Key,
    pub level: f64,
    pub deflection: f64,
    /// Its tight box, if it was measured.
    pub bounds: Option<[[f64; 3]; 2]>,
    /// The half pixels `[at_least, below)` its mesh serves.
    pub serves: [f64; 2],
    pub mesh: Arc<Mesh>,
    /// Its faces and edges, in the mesh's order, each named (`Face1` is the first).
    pub faces: Vec<MeshElement>,
    pub edges: Vec<MeshElement>,
}

/// What the viewport was last frame, for what picks in it (M5.7): where it was in the
/// window, in points; the camera; the size in physical pixels; the display's scale; and
/// the pointer, in physical pixels from the viewport's top left, if it's over the viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewFrame {
    pub rect: Rect,
    pub camera: Camera,
    pub size_px: [u32; 2],
    pub pixels_per_point: f32,
    pub pointer_px: Option<[f32; 2]>,
}

/// What a `meshes` read was asked under: the published revision, the camera, and the size.
#[derive(Debug, Clone, PartialEq)]
struct Asked {
    revision: Revision,
    camera: Camera,
    size: [u32; 2],
}

/// The `meshes` read in flight.
struct Reading {
    running: Running<MeshesRead>,
    asked: Asked,
    /// Asked for Home's view, fitted by the core, whose camera the tab then takes.
    named: bool,
}

/// What a left drag does, as the navigation bar's tools set it (fusion.md, *ViewCube and
/// Navigation Bar*). With none chosen, it's the selection's (M5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Orbit,
    Pan,
    Zoom,
}

/// What a drag in the viewport does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Navigation {
    Orbit,
    Pan,
    Zoom,
}

/// A tab's viewport: its camera, its meshes, and its read.
pub struct View {
    camera: Camera,
    /// Whether the camera was chosen: by the person, or by the first read's Home fit. Until
    /// then, a read asks for Home's view of the bodies, and the tab takes its camera.
    placed: bool,
    /// The bodies by number, as the newest answer gave them, and the revision it read.
    bodies: BTreeMap<u32, ViewBody>,
    model: Option<Revision>,
    reading: Option<Reading>,
    /// The last read refused, as it was asked: not asked again until something differs.
    refused: Option<Asked>,
    /// A held mesh an answer gave none of wasn't found: the next read holds none.
    hold_none: bool,
    reads: u64,
    held_asked: Vec<Held>,
    /// The camera and size of the last frame, to tell when the view has settled.
    last: Option<(Camera, [u32; 2])>,
    /// A drag under way: its button, and where the pointer was last frame.
    drag: Option<(PointerButton, Pos2)>,
    /// The navigation bar's tool, if one is chosen.
    pub tool: Option<Tool>,
    /// The elements drawn highlighted, set from outside (M5.7's selection): passed through.
    pub highlights: BTreeSet<(u32, Element)>,
    picking: Option<PixelRect>,
    picked: Option<Picking>,
    frame: Option<ViewFrame>,
}

impl Default for View {
    fn default() -> Self {
        View {
            camera: HOME.camera(NO_BOX, 1, 1),
            placed: false,
            bodies: BTreeMap::new(),
            model: None,
            reading: None,
            refused: None,
            hold_none: false,
            reads: 0,
            held_asked: Vec::new(),
            last: None,
            drag: None,
            tool: None,
            highlights: BTreeSet::new(),
            picking: None,
            picked: None,
            frame: None,
        }
    }
}

/// The half pixel, in mm, `camera` gives a body with box `bounds` in an image `height`
/// pixels high: at its meshing depth, or at none with no box, as the read takes it
/// (`command/meshes.rs`, `level_of`), so the app reads again by the core's own rule.
pub fn half_pixel(camera: &Camera, height: u32, bounds: Option<[[f64; 3]; 2]>) -> Option<f64> {
    let depth = bounds.map_or(0.0, |bounds| meshing_depth(camera, bounds));
    half_pixel_at(camera, height, depth)
}

/// Whether any of `bodies` has its half pixel, under `camera` in an image `height` pixels
/// high, outside the half pixels its mesh serves: a read would mesh it otherwise.
pub fn outside_serves<'a>(
    camera: &Camera,
    height: u32,
    bodies: impl IntoIterator<Item = &'a ViewBody>,
) -> bool {
    bodies.into_iter().any(|body| {
        half_pixel(camera, height, body.bounds)
            .is_some_and(|half| !(body.serves[0] <= half && half < body.serves[1]))
    })
}

/// The size a `meshes` read asks for: the viewport's, each side from 1 to
/// [`MAX_RENDER_SIDE`], the most a read takes.
pub fn read_size(size_px: [u32; 2]) -> [u32; 2] {
    size_px.map(|side| side.clamp(1, MAX_RENDER_SIDE))
}

impl View {
    /// The camera.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// Looks through `camera`, as a control of the person's does.
    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
        self.placed = true;
    }

    /// The bodies whose meshes the tab holds, by number.
    pub fn bodies(&self) -> &BTreeMap<u32, ViewBody> {
        &self.bodies
    }

    /// The published revision the bodies were read from, once a read has answered.
    pub fn model(&self) -> Option<Revision> {
        self.model
    }

    /// How many `meshes` reads the tab has started.
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// The meshes the last read listed as held.
    pub fn held_asked(&self) -> &[Held] {
        &self.held_asked
    }

    /// Whether a `meshes` read is in flight.
    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }

    /// What the viewport was last frame (M5.7).
    pub fn frame(&self) -> Option<&ViewFrame> {
        self.frame.as_ref()
    }

    /// Asks the next frame to pick under `rect`, if a renderer takes it
    /// ([`PixelRect::takes`]): its answer comes a frame after ([`picked`](Self::picked)).
    /// Whether it was asked.
    pub fn pick(&mut self, rect: PixelRect) -> bool {
        let takes = rect.takes();
        if takes {
            self.picking = Some(rect);
        }
        takes
    }

    /// The newest picking answer.
    pub fn picked(&self) -> Option<Picking> {
        self.picked
    }

    /// The read in flight polled: its answer taken if it has come.
    fn poll(&mut self) {
        let Some(reading) = self.reading.take() else {
            return;
        };
        match reading.running.try_take() {
            Ok(response) => self.took(reading.asked, reading.named, response),
            Err(running) => {
                self.reading = Some(Reading { running, ..reading });
            }
        }
    }

    /// A read's answer: its bodies taken (and, for the first, Home's camera), or its
    /// refusal kept so the same read isn't asked again.
    fn took(&mut self, asked: Asked, named: bool, response: Response<MeshesRead>) {
        match response.result {
            Ok(read) => {
                if named && !self.placed {
                    self.camera = read.camera.camera();
                }
                self.placed = true;
                self.refused = None;
                self.model = Some(
                    response
                        .model
                        .map_or(asked.revision, |model| model.revision),
                );
                self.take_bodies(read.bodies);
            }
            // Home fits the bodies by their boxes: with none to fit (no body, or one that
            // couldn't be measured), the tab keeps its camera and reads with it.
            Err(problems)
                if named
                    && !self.placed
                    && problems
                        .errors()
                        .iter()
                        .any(|error| error.parameter() == Some("view")) =>
            {
                self.placed = true;
            }
            Err(_) => self.refused = Some(asked),
        }
    }

    /// The table made again from an answer's bodies: each body's mesh the answer's, or, for
    /// one held, the one the tab holds of its key at its deflection. A held one the tab
    /// hasn't (which a read never answers) is left out, and the next read holds none.
    fn take_bodies(&mut self, bodies: Vec<MeshedBody>) {
        let held: BTreeMap<(Key, u64), Arc<Mesh>> = self
            .bodies
            .values()
            .map(|body| {
                (
                    (body.key, body.deflection.to_bits()),
                    Arc::clone(&body.mesh),
                )
            })
            .collect();
        let mut table = BTreeMap::new();
        for body in bodies {
            let Some(number) = body.body else {
                continue;
            };
            let mesh = match body.mesh {
                Some(mesh) => mesh,
                None => match held.get(&(body.key, body.deflection.to_bits())) {
                    Some(mesh) => Arc::clone(mesh),
                    None => {
                        self.hold_none = true;
                        self.model = None;
                        continue;
                    }
                },
            };
            table.insert(
                number,
                ViewBody {
                    feature: body.feature,
                    key: body.key,
                    level: body.level,
                    deflection: body.deflection,
                    bounds: body.bounds,
                    serves: body.serves,
                    mesh,
                    faces: body.faces,
                    edges: body.edges,
                },
            );
        }
        self.bodies = table;
    }

    /// The read to start, if one is wanted (section 3, *When the app reads*): none while
    /// one is in flight or nothing is published; else one when the published revision isn't
    /// the one the bodies were read from, or when the view has `settled` with a body's half
    /// pixel outside its `serves`. A read refused isn't asked again as it was, and after a
    /// refusal of this revision, only once the view has settled.
    fn wanted(&self, published: Option<Revision>, size: [u32; 2], settled: bool) -> Option<Asked> {
        if self.reading.is_some() {
            return None;
        }
        let asked = Asked {
            revision: published?,
            camera: self.camera.clone(),
            size,
        };
        if let Some(refused) = &self.refused
            && refused.revision == asked.revision
            && (!settled || *refused == asked)
        {
            return None;
        }
        let stale = self.model != Some(asked.revision);
        let outside = settled && outside_serves(&self.camera, size[1], self.bodies.values());
        (stale || outside).then_some(asked)
    }

    /// Starts the read `asked`, of the published model, listing every mesh held.
    fn start<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        asked: Asked,
    ) {
        let named = !self.placed;
        let held: Vec<Held> = if std::mem::take(&mut self.hold_none) {
            Vec::new()
        } else {
            self.bodies
                .values()
                .map(|body| Held {
                    key: body.key,
                    deflection: body.deflection,
                })
                .collect()
        };
        let request = Meshes {
            view: named.then_some(HOME),
            camera: (!named).then(|| asked.camera.clone()),
            width: asked.size[0],
            height: asked.size[1],
            features: None,
            held: held.clone(),
        };
        self.held_asked = held;
        self.reads += 1;
        self.reading = Some(Reading {
            running: sessions.meshes(session, ModelChoice::Published, request),
            asked,
            named,
        });
    }

    /// The camera moved to `camera`, if a control made one.
    fn moved(&mut self, camera: Option<Camera>) {
        if let Some(camera) = camera {
            self.camera = camera;
            self.placed = true;
        }
    }

    /// Every box the tab holds, joined.
    fn bounds(&self) -> Option<[[f64; 3]; 2]> {
        navigate::joined(self.bodies.values().filter_map(|body| body.bounds))
    }

    /// The named view `view` of the bodies' boxes (a cube's, with none).
    fn look_from(&mut self, view: NamedView, size_px: [u32; 2]) {
        let bounds = self.bounds().unwrap_or(NO_BOX);
        let camera = navigate::named(view, bounds, size_px[0], size_px[1]);
        self.moved(camera);
    }

    /// Fits the view to the bodies' boxes (F6), looking as it looks.
    fn fit(&mut self, size_px: [u32; 2]) {
        if let Some(bounds) = self.bounds() {
            let camera = navigate::fit(&self.camera, bounds, size_px[0], size_px[1]);
            self.moved(camera);
        }
    }

    /// A drag of `delta` points, in a view `height` points high, as `navigation` takes it.
    fn navigated(&mut self, navigation: Navigation, delta: Vec2, height: f64) {
        let (dx, dy) = (f64::from(delta.x), f64::from(delta.y));
        let camera = match navigation {
            Navigation::Orbit => navigate::orbit(&self.camera, dx, dy),
            Navigation::Pan => navigate::pan(&self.camera, dx, dy, height),
            // A drag up zooms in, about the view's centre.
            Navigation::Zoom => navigate::zoom(&self.camera, -dy, [0.0, 0.0], height),
        };
        self.moved(camera);
    }

    /// The person's navigation this frame (section 8, *Navigation*, Fusion's preset): the
    /// wheel zooms toward the cursor; the middle button pans, and orbits with Shift; a left
    /// drag does the navigation bar's tool, if one is chosen; F6, or a double-click of the
    /// middle button, fits.
    fn navigate(&mut self, ui: &mut Ui, response: &egui::Response, size_px: [u32; 2]) {
        let rect = response.rect;
        let height = f64::from(rect.height());
        let (pressed, origin, latest, shift, down) = ui.input(|input| {
            let pointer = &input.pointer;
            let pressed = if pointer.button_pressed(PointerButton::Middle) {
                Some(PointerButton::Middle)
            } else if pointer.button_pressed(PointerButton::Primary) {
                Some(PointerButton::Primary)
            } else {
                None
            };
            (
                pressed,
                pointer.press_origin(),
                pointer.latest_pos(),
                input.modifiers.shift,
                [
                    pointer.button_down(PointerButton::Middle),
                    pointer.button_down(PointerButton::Primary),
                ],
            )
        });
        if self.drag.is_none()
            && response.hovered()
            && let (Some(button), Some(origin)) = (pressed, origin)
            && (button == PointerButton::Middle || self.tool.is_some())
        {
            self.drag = Some((button, origin));
        }
        if let Some((button, last)) = self.drag {
            let navigation = match (button, self.tool) {
                (PointerButton::Middle, _) if shift => Some(Navigation::Orbit),
                (PointerButton::Middle, _) => Some(Navigation::Pan),
                (_, Some(Tool::Orbit)) => Some(Navigation::Orbit),
                (_, Some(Tool::Pan)) => Some(Navigation::Pan),
                (_, Some(Tool::Zoom)) => Some(Navigation::Zoom),
                (_, None) => None,
            };
            let at = latest.unwrap_or(last);
            if let Some(navigation) = navigation
                && at != last
            {
                self.navigated(navigation, at - last, height);
            }
            let held = match button {
                PointerButton::Middle => down[0],
                _ => down[1],
            };
            self.drag = held.then_some((button, at));
        }
        if response.hovered() {
            let line = ui
                .ctx()
                .options(|options| options.input_options.line_scroll_speed);
            let scroll: f32 = ui.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                            egui::MouseWheelUnit::Point => delta.y,
                            egui::MouseWheelUnit::Line => delta.y * line,
                            egui::MouseWheelUnit::Page => delta.y * rect.height(),
                        }),
                        _ => None,
                    })
                    .sum()
            });
            if scroll != 0.0
                && let Some(pointer) = latest
            {
                let cursor = pointer - rect.center();
                let camera = navigate::zoom(
                    &self.camera,
                    f64::from(scroll),
                    [f64::from(cursor.x), f64::from(cursor.y)],
                    height,
                );
                self.moved(camera);
            }
        }
        let f6 = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F6));
        if f6 || response.double_clicked_by(PointerButton::Middle) {
            self.fit(size_px);
        }
    }
}

/// What the renderer holds, for the tab shown: each body by number, with the session, the
/// key, and the deflection its mesh was uploaded for; each body whose upload it refused, as
/// it was asked; and how frames reach it.
#[derive(Default)]
pub struct Drawing {
    present: Option<Box<dyn Present>>,
    uploaded: BTreeMap<u32, (SessionId, Key, u64)>,
    refused: BTreeMap<u32, (SessionId, Key, u64)>,
}

impl Drawing {
    /// Frames drawn through `present`.
    pub fn new(present: Box<dyn Present>) -> Self {
        Drawing {
            present: Some(present),
            uploaded: BTreeMap::new(),
            refused: BTreeMap::new(),
        }
    }

    /// Whether body `number` is held by the renderer, to be drawn.
    pub fn holds(&self, number: u32) -> bool {
        self.uploaded.contains_key(&number)
    }

    /// The bodies the renderer refused since the last frame: not drawn, and not uploaded
    /// again until their mesh changes (a refusal for want of memory would be again).
    fn take_refused(&mut self) {
        let refused = match &mut self.present {
            Some(present) => present.refused(),
            None => return,
        };
        for number in refused {
            if let Some(asked) = self.uploaded.remove(&number) {
                self.refused.insert(number, asked);
            }
        }
    }

    /// The changes that make the renderer hold `bodies` of session `session`'s tab (none for
    /// no tab): a body uploaded when its key or its deflection differs from what was
    /// uploaded for it, or it's another tab's; a body not among them forgotten. So each body
    /// is uploaded once for each mesh it's drawn with, whatever the frames between.
    pub fn changes(
        &mut self,
        session: Option<SessionId>,
        bodies: &BTreeMap<u32, ViewBody>,
    ) -> Vec<Change> {
        let mut changes = Vec::new();
        // A body refused is forgotten too when it goes: the renderer may hold an older mesh
        // of it.
        let gone: BTreeSet<u32> = self
            .uploaded
            .keys()
            .chain(self.refused.keys())
            .filter(|number| session.is_none() || !bodies.contains_key(number))
            .copied()
            .collect();
        for number in gone {
            self.uploaded.remove(&number);
            self.refused.remove(&number);
            changes.push(Change::Forget(number));
        }
        if let Some(session) = session {
            for (number, body) in bodies {
                let wanted = (session, body.key, body.deflection.to_bits());
                if self.refused.get(number) == Some(&wanted) {
                    continue;
                }
                self.refused.remove(number);
                if self.uploaded.get(number) != Some(&wanted) {
                    self.uploaded.insert(*number, wanted);
                    changes.push(Change::Upload(*number, Arc::clone(&body.mesh)));
                }
            }
        }
        changes
    }

    fn present(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        changes: Vec<Change>,
        frame: Option<FrameData>,
    ) {
        if let Some(present) = &mut self.present {
            present.present(painter, rect, changes, frame);
        }
    }

    fn picked(&mut self) -> Option<Picking> {
        self.present.as_mut().and_then(|present| present.picked())
    }
}

/// A side in physical pixels, from a length in them.
fn pixels(length: f32) -> u32 {
    // A pane's side is a few thousand pixels at most; the cast saturates past u32 anyway.
    length.round().max(0.0) as u32
}

/// The viewport pane: the tab's view drawn, its navigation taken, its read started when
/// one is wanted, and its frame given to the renderer.
pub fn show<P: Clients + 'static>(
    ui: &mut Ui,
    tab: Option<&mut Tab>,
    sessions: &Arc<Sessions<P>>,
    drawing: &mut Drawing,
) {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let size_px = [pixels(rect.width() * ppp), pixels(rect.height() * ppp)];
    let Some(tab) = tab else {
        let changes = drawing.changes(None, &BTreeMap::new());
        drawing.present(ui.painter(), rect, changes, None);
        return;
    };
    let hidden: BTreeSet<&FeatureId> = tab
        .browser
        .bodies
        .iter()
        .filter(|row| tab.hidden.contains(&row.name))
        .map(|row| &row.id)
        .collect();
    let view = &mut tab.view;
    view.poll();
    if let Some(picking) = drawing.picked() {
        view.picked = Some(picking);
    }
    drawing.take_refused();
    view.navigate(ui, &response, size_px);
    view_cube(ui, rect, view, size_px);
    navigation_bar(ui, rect, view, size_px);
    let size = read_size(size_px);
    let settled = view.drag.is_none() && view.last.as_ref() == Some(&(view.camera.clone(), size));
    if !settled {
        // A frame after this one, to see the view settled.
        ui.ctx().request_repaint();
    }
    if let Some(asked) = view.wanted(tab.published, size, settled) {
        view.start(sessions, &tab.id, asked);
    }
    view.last = Some((view.camera.clone(), size));
    let changes = drawing.changes(Some(tab.id), &view.bodies);
    let pointer_px = ui
        .input(|input| input.pointer.latest_pos())
        .filter(|at| rect.contains(*at))
        .map(|at| [(at.x - rect.left()) * ppp, (at.y - rect.top()) * ppp]);
    view.frame = Some(ViewFrame {
        rect,
        camera: view.camera.clone(),
        size_px,
        pixels_per_point: ppp,
        pointer_px,
    });
    // Ascending, each once, as the renderer takes them: the table's order.
    let frame = FrameData {
        camera: view.camera.clone(),
        size_px,
        pixels_per_point: ppp,
        bodies: view
            .bodies
            .iter()
            .filter(|(number, body)| !hidden.contains(&body.feature) && drawing.holds(**number))
            .map(|(number, _)| *number)
            .collect(),
        highlights: view.highlights.iter().copied().collect(),
        picking: view.picking.take(),
    };
    drawing.present(ui.painter(), rect, changes, Some(frame));
}

/// The ViewCube's faces: each named view's face, its outward normal.
const FACES: [(NamedView, [f64; 3]); 6] = [
    (NamedView::Front, [0.0, -1.0, 0.0]),
    (NamedView::Back, [0.0, 1.0, 0.0]),
    (NamedView::Left, [-1.0, 0.0, 0.0]),
    (NamedView::Right, [1.0, 0.0, 0.0]),
    (NamedView::Top, [0.0, 0.0, 1.0]),
    (NamedView::Bottom, [0.0, 0.0, -1.0]),
];

/// A named view's words, as the ViewCube labels it: `top_front_right` is "Top front right".
fn words(view: NamedView) -> String {
    let spaced = view.name().replace('_', " ");
    let mut chars = spaced.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A clickable part of the ViewCube, labelled for the accessibility tree.
fn cube_part(ui: &mut Ui, rect: Rect, label: &str) -> bool {
    let response = ui.interact(rect, ui.id().with(("viewcube", label)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    response.on_hover_text(label).clicked()
}

/// The ViewCube, at the canvas's top right (section 8; fusion.md, *ViewCube and Navigation
/// Bar*): the cube as the camera sees it; a face's click gives its orthographic view, a
/// corner's its isometric one, a drag orbits, and Home gives the home view; its menu
/// switches the projection.
fn view_cube(ui: &mut Ui, viewport: Rect, view: &mut View, size_px: [u32; 2]) {
    const SIDE: f32 = 96.0;
    const MARGIN: f32 = 8.0;
    let rect = Rect::from_min_size(
        egui::pos2(
            viewport.right() - SIDE - MARGIN,
            viewport.top() + MARGIN + 20.0,
        ),
        Vec2::splat(SIDE),
    );
    let Ok(frame) = view.camera.frame() else {
        return;
    };
    ui.scope_builder(UiBuilder::new().id_salt("viewcube"), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::Group);
            node.set_label("ViewCube");
        });
        let cube = ui.interact(rect, ui.id().with("cube"), Sense::click_and_drag());
        if cube.dragged() {
            let delta = cube.drag_delta();
            let camera = navigate::orbit(&view.camera, f64::from(delta.x), f64::from(delta.y));
            view.moved(camera);
        }
        cube.context_menu(|ui| {
            let perspective = matches!(view.camera.projection, Projection::Perspective { .. });
            for (word, wanted) in [("Orthographic", false), ("Perspective", true)] {
                if ui.radio(perspective == wanted, word).clicked() {
                    let camera = navigate::projected(&view.camera, wanted);
                    view.moved(camera);
                    ui.close();
                }
            }
        });
        let scale = f64::from(SIDE) * 0.28;
        let at = |v: [f64; 3]| {
            let x = silico_protocol::vec3::dot(v, frame.right) * scale;
            let y = -silico_protocol::vec3::dot(v, frame.up) * scale;
            let offset = Vec2::new(x as f32, y as f32);
            rect.center() + offset
        };
        let painter = ui.painter_at(rect);
        let fill = ui.visuals().widgets.inactive.bg_fill;
        let stroke = Stroke::new(1.0, ui.visuals().widgets.inactive.fg_stroke.color);
        let mut chosen = None;
        for (named, normal) in FACES {
            let facing = silico_protocol::vec3::dot(normal, frame.back);
            if facing <= 1e-9 {
                continue;
            }
            // The face's two directions across it, and its corners in turn about it.
            let k = normal.iter().position(|c| *c != 0.0).unwrap_or(0);
            let (a, b) = ((k + 1) % 3, (k + 2) % 3);
            let corner = |sa: f64, sb: f64| {
                let mut v = normal;
                v[a] = sa;
                v[b] = sb;
                at(v)
            };
            let points = vec![
                corner(1.0, 1.0),
                corner(1.0, -1.0),
                corner(-1.0, -1.0),
                corner(-1.0, 1.0),
            ];
            let shade = fill.gamma_multiply(0.6 + 0.4 * facing as f32);
            painter.add(egui::Shape::convex_polygon(points, shade, stroke));
            if facing > 0.25 {
                let label = words(named);
                let centre = at(normal);
                painter.text(
                    centre,
                    egui::Align2::CENTER_CENTER,
                    &label,
                    egui::FontId::proportional(10.0),
                    ui.visuals().text_color(),
                );
                let part = Rect::from_center_size(centre, Vec2::splat(SIDE * 0.3));
                if cube_part(ui, part, &label) {
                    chosen = Some(named);
                }
            }
        }
        for named in silico_core::views::NAMED_VIEWS {
            let direction = named.direction();
            if FACES.iter().any(|(face, _)| *face == named)
                || silico_protocol::vec3::dot(direction, frame.back) <= 0.1
            {
                continue;
            }
            let corner = at(direction.map(|c| c * 3f64.sqrt()));
            painter.circle_filled(corner, 3.0, stroke.color);
            if cube_part(
                ui,
                Rect::from_center_size(corner, Vec2::splat(12.0)),
                &words(named),
            ) {
                chosen = Some(named);
            }
        }
        let home = Rect::from_min_size(
            rect.left_top() - Vec2::new(0.0, 20.0),
            Vec2::new(44.0, 18.0),
        );
        if ui.put(home, egui::Button::new("Home").small()).clicked() {
            chosen = Some(HOME);
        }
        if let Some(named) = chosen {
            view.look_from(named, size_px);
        }
    });
}

/// The navigation bar, floating at the canvas's bottom centre (section 8): Orbit, Pan, and
/// Zoom, each the left drag's tool while chosen, and Fit.
fn navigation_bar(ui: &mut Ui, viewport: Rect, view: &mut View, size_px: [u32; 2]) {
    let size = Vec2::new(220.0, 26.0);
    let rect = Rect::from_center_size(
        egui::pos2(viewport.center().x, viewport.bottom() - size.y / 2.0 - 8.0),
        size,
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("navigation_bar")
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Toolbar);
                node.set_label("Navigation bar");
            });
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .corner_radius(crate::theme::SMALL_CORNER_RADIUS)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (tool, word) in [
                            (Tool::Orbit, "Orbit"),
                            (Tool::Pan, "Pan"),
                            (Tool::Zoom, "Zoom"),
                        ] {
                            let on = view.tool == Some(tool);
                            if ui.selectable_label(on, word).clicked() {
                                view.tool = (!on).then_some(tool);
                            }
                        }
                        if ui.button("Fit").clicked() {
                            view.fit(size_px);
                        }
                    });
                });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From −y onto (20, 10, 5), `view_height` high.
    fn front(view_height: f64) -> Camera {
        Camera {
            projection: Projection::Orthographic { view_height },
            eye: [20.0, -100.0, 5.0],
            target: [20.0, 10.0, 5.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    fn key(byte: u8) -> Key {
        Key::parse(&format!("{byte:02x}").repeat(32)).unwrap()
    }

    fn body(level: f64, serves: [f64; 2], bounds: Option<[[f64; 3]; 2]>) -> ViewBody {
        ViewBody {
            feature: FeatureId::new("m3tq8v0z").unwrap(),
            key: key(1),
            level,
            deflection: level,
            bounds,
            serves,
            mesh: Arc::new(Mesh {
                positions: Vec::new(),
                normals: Vec::new(),
                triangles: Vec::new(),
                face_triangles: Vec::new(),
                valid: None,
                edge_points: Vec::new(),
                edge_point_counts: Vec::new(),
            }),
            faces: Vec::new(),
            edges: Vec::new(),
        }
    }

    const CUT: Option<[[f64; 3]; 2]> = Some([[0.0, 0.0, -1.0], [40.0, 20.0, 11.0]]);

    #[test]
    fn an_orthographic_half_pixel_is_the_views_height_over_twice_its_pixels_at_any_depth() {
        // 80 mm over 1,000 pixels: 0.04 mm, the design's example, box or no box.
        assert_eq!(half_pixel(&front(80.0), 1000, CUT), Some(0.04));
        assert_eq!(half_pixel(&front(80.0), 1000, None), Some(0.04));
    }

    #[test]
    fn an_orthographic_view_reads_again_only_once_its_half_pixel_leaves_serves() {
        // Level 2⁻⁵ serves [0.03125, 0.0625): at 0.04, 0.03125 exactly, and just under
        // 0.0625 it's served; at 0.0625, and under 0.03125, it isn't. Half pixels by hand:
        // view heights 80, 62.5, and 124.99 over 1,000 pixels, then 125 and 62.4.
        let held = [body(0.03125, [0.03125, 0.0625], CUT)];
        for (height, served) in [
            (80.0, true),
            (62.5, true),
            (124.99, true),
            (125.0, false),
            (62.4, false),
        ] {
            assert_eq!(
                outside_serves(&front(height), 1000, &held),
                !served,
                "{height} mm high"
            );
        }
    }

    #[test]
    fn a_perspective_view_takes_each_bodys_half_pixel_at_its_nearest_depth() {
        // The design's case (section 13, M5.5's `serves`): the bracket's box (0, 0, 0) to
        // (60, 40, 50) from (100, −100, 100) toward (30, 20, 25), 30°, 1,024 pixels high:
        // the nearest corner's depth is 117.497 mm, its half pixel 0.030745 by hand, served
        // by level 2⁻⁶'s [0.015625, 0.03125); the box's centre would give 0.041311, which
        // isn't.
        let camera = Camera {
            projection: Projection::Perspective {
                field_of_view: 30.0,
            },
            eye: [100.0, -100.0, 100.0],
            target: [30.0, 20.0, 25.0],
            up: [0.0, 0.0, 1.0],
        };
        let bracket = Some([[0.0, 0.0, 0.0], [60.0, 40.0, 50.0]]);
        let half = half_pixel(&camera, 1024, bracket).unwrap();
        assert!((half - 0.030745).abs() < 5e-7, "{half}");
        let held = [body(0.015625, [0.015625, 0.03125], bracket)];
        assert!(!outside_serves(&camera, 1024, &held));
        let coarser = [body(0.03125, [0.03125, 0.0625], bracket)];
        assert!(outside_serves(&camera, 1024, &coarser));
    }

    #[test]
    fn a_body_with_no_box_is_read_again_by_the_views_half_pixel_or_never_in_perspective() {
        // `box: null`. Orthographic: its level is the view's, as every body's, so it leaves
        // [0.03125, 0.0625) when the view does. Perspective: at no depth its level is its
        // floor, serving [0, 2 · mesh_level(floor)), and its half pixel is 0 at no depth,
        // so no move of the view reads it again.
        let unmeasured = [body(0.03125, [0.03125, 0.0625], None)];
        assert!(!outside_serves(&front(80.0), 1000, &unmeasured));
        assert!(outside_serves(&front(40.0), 1000, &unmeasured));
        let at_floor = [body(0.0087750, [0.0, 0.015625], None)];
        for eye in [[100.0, -100.0, 100.0], [1e5, -1e5, 1e5], [30.0, 19.0, 25.0]] {
            let camera = Camera {
                projection: Projection::Perspective {
                    field_of_view: 30.0,
                },
                eye,
                target: [30.0, 20.0, 25.0],
                up: [0.0, 0.0, 1.0],
            };
            assert_eq!(half_pixel(&camera, 1024, None), Some(0.0));
            assert!(!outside_serves(&camera, 1024, &at_floor), "{eye:?}");
        }
    }

    fn view_with(bodies: &[(u32, ViewBody)], model: u64) -> View {
        View {
            camera: front(80.0),
            placed: true,
            bodies: bodies.iter().cloned().collect(),
            model: Revision::from_number(model),
            ..View::default()
        }
    }

    fn revision(n: u64) -> Option<Revision> {
        Revision::from_number(n)
    }

    #[test]
    fn a_read_is_wanted_for_a_new_published_revision_and_for_a_settled_view_outside_serves() {
        let held = [(1, body(0.03125, [0.03125, 0.0625], CUT))];
        let mut view = view_with(&held, 3);
        // Nothing published: nothing to read.
        assert_eq!(view.wanted(None, [1000, 1000], true), None);
        // The bodies are of revision 3: none at 3, one at 4, settled or not.
        assert_eq!(view.wanted(revision(3), [1000, 1000], true), None);
        assert!(view.wanted(revision(4), [1000, 1000], false).is_some());
        // Zoomed in to 40 mm: 0.02 leaves serves, but only a settled view reads.
        view.camera = front(40.0);
        assert_eq!(view.wanted(revision(3), [1000, 1000], false), None);
        let asked = view.wanted(revision(3), [1000, 1000], true).unwrap();
        assert_eq!(asked.camera, front(40.0));
        assert_eq!(asked.size, [1000, 1000]);
        // A narrower window changes nothing orthographic but its height: 2,000 pixels high
        // at 80 mm halves the half pixel, out of serves too.
        view.camera = front(80.0);
        assert_eq!(view.wanted(revision(3), [500, 1000], true), None);
        assert!(view.wanted(revision(3), [1000, 2000], true).is_some());
    }

    #[test]
    fn a_refused_read_is_not_asked_again_until_the_view_differs_and_settles() {
        let held = [(1, body(0.03125, [0.03125, 0.0625], CUT))];
        let mut view = view_with(&held, 3);
        let asked = view.wanted(revision(4), [1000, 1000], true).unwrap();
        view.refused = Some(asked);
        assert_eq!(view.wanted(revision(4), [1000, 1000], true), None);
        // Moving, not settled: still none (a drag would read each frame).
        view.camera = front(81.0);
        assert_eq!(view.wanted(revision(4), [1000, 1000], false), None);
        // Settled elsewhere: once more; and a newer revision at once.
        assert!(view.wanted(revision(4), [1000, 1000], true).is_some());
        assert!(view.wanted(revision(5), [1000, 1000], false).is_some());
    }

    fn meshed(number: u32, key_byte: u8, deflection: f64, mesh: Option<Arc<Mesh>>) -> MeshedBody {
        MeshedBody {
            feature: FeatureId::new("m3tq8v0z").unwrap(),
            body: Some(number),
            key: key(key_byte),
            level: deflection,
            deflection,
            bounds: CUT,
            serves: [deflection, 2.0 * deflection],
            held: mesh.is_none(),
            mesh,
            faces: Vec::new(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn a_held_body_keeps_the_mesh_the_tab_holds_of_its_key_at_its_deflection() {
        let mut view = view_with(&[(1, body(0.03125, [0.03125, 0.0625], CUT))], 3);
        let mesh = Arc::clone(&view.bodies[&1].mesh);
        // Renumbered (a body before it, new): the same mesh, by its key and deflection.
        view.take_bodies(vec![meshed(2, 1, 0.03125, None)]);
        assert!(Arc::ptr_eq(&view.bodies[&2].mesh, &mesh));
        assert!(!view.bodies.contains_key(&1));
        assert!(!view.hold_none);
        // Held at another deflection, which the tab hasn't: left out, and the next read
        // holds nothing, and is wanted at once.
        view.take_bodies(vec![meshed(2, 1, 0.0625, None)]);
        assert!(view.bodies.is_empty());
        assert!(view.hold_none);
        assert_eq!(view.model, None);
    }

    #[test]
    fn the_renderer_is_told_of_each_body_once_for_each_mesh_and_of_a_body_gone() {
        let mut drawing = Drawing::default();
        let s1 = SessionId::parse("s1").unwrap();
        let s2 = SessionId::parse("s2").unwrap();
        let mut bodies: BTreeMap<u32, ViewBody> = [
            (1, body(0.03125, [0.03125, 0.0625], CUT)),
            (2, body(0.03125, [0.03125, 0.0625], CUT)),
        ]
        .into_iter()
        .collect();
        let told = |changes: Vec<Change>| -> Vec<String> {
            changes
                .into_iter()
                .map(|change| match change {
                    Change::Upload(n, _) => format!("upload {n}"),
                    Change::Forget(n) => format!("forget {n}"),
                })
                .collect()
        };
        assert_eq!(
            told(drawing.changes(Some(s1), &bodies)),
            ["upload 1", "upload 2"]
        );
        for _ in 0..100 {
            assert!(drawing.changes(Some(s1), &bodies).is_empty());
        }
        // Body 2 at another level: it alone, again.
        bodies.get_mut(&2).unwrap().deflection = 0.015625;
        assert_eq!(told(drawing.changes(Some(s1), &bodies)), ["upload 2"]);
        // At another key, the same level: again.
        bodies.get_mut(&1).unwrap().key = key(7);
        assert_eq!(told(drawing.changes(Some(s1), &bodies)), ["upload 1"]);
        // Body 1 gone.
        bodies.remove(&1);
        assert_eq!(told(drawing.changes(Some(s1), &bodies)), ["forget 1"]);
        // Another tab's body 2, the same key and level: its own, so uploaded.
        assert_eq!(told(drawing.changes(Some(s2), &bodies)), ["upload 2"]);
        // No tab: everything forgotten.
        assert_eq!(told(drawing.changes(None, &bodies)), ["forget 2"]);
    }

    /// A present that refuses the bodies it's told to.
    struct Refusing(BTreeSet<u32>);

    impl Present for Refusing {
        fn present(&mut self, _: &egui::Painter, _: Rect, _: Vec<Change>, _: Option<FrameData>) {}
        fn picked(&mut self) -> Option<Picking> {
            None
        }
        fn refused(&mut self) -> BTreeSet<u32> {
            std::mem::take(&mut self.0)
        }
    }

    #[test]
    fn a_body_whose_upload_was_refused_isnt_held_or_uploaded_again_until_its_mesh_changes() {
        let mut drawing = Drawing::new(Box::new(Refusing([2].into_iter().collect())));
        let s1 = SessionId::parse("s1").unwrap();
        let mut bodies: BTreeMap<u32, ViewBody> = [
            (1, body(0.03125, [0.03125, 0.0625], CUT)),
            (2, body(0.03125, [0.03125, 0.0625], CUT)),
        ]
        .into_iter()
        .collect();
        assert_eq!(drawing.changes(Some(s1), &bodies).len(), 2);
        drawing.take_refused();
        assert!(drawing.holds(1) && !drawing.holds(2));
        assert!(drawing.changes(Some(s1), &bodies).is_empty());
        // Its level changes: tried again.
        bodies.get_mut(&2).unwrap().deflection = 0.015625;
        let again = drawing.changes(Some(s1), &bodies);
        assert!(matches!(again[..], [Change::Upload(2, _)]));
        assert!(drawing.holds(2));
        // Refused again, then gone: forgotten, as the renderer may hold an older mesh.
        drawing.present = Some(Box::new(Refusing([2].into_iter().collect())));
        drawing.take_refused();
        bodies.remove(&2);
        let gone = drawing.changes(Some(s1), &bodies);
        assert!(matches!(gone[..], [Change::Forget(2)]));
    }

    #[test]
    fn a_read_asks_at_most_the_largest_side_a_read_takes() {
        assert_eq!(read_size([0, 5000]), [1, 4096]);
        assert_eq!(read_size([1600, 1000]), [1600, 1000]);
    }

    #[test]
    fn the_view_cubes_labels_are_the_named_views_in_words() {
        assert_eq!(words(NamedView::TopFrontRight), "Top front right");
        assert_eq!(words(NamedView::Front), "Front");
    }
}
