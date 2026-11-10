//! Select Other (M5.7; docs/design/m5.0-app.md, section 9's *Selection*): a right click on the
//! canvas opens its context menu (`crate::canvas_menu`), *Select Other…* among its items, and
//! that a list of everything under the pointer, nearest the eye first, as SolidWorks lists it
//! (docs/ux/solidworks.md §1, *Select Other*) and the approved prototype draws it
//! (docs/design/m5-ui: DESIGN.md's *Menus*, interact.js's `onContext` and `openSelectOther`).
//!
//! **The cast.** The right click casts at its pixel's centre, with that frame's camera and
//! size, on the bodies that frame drew (none the browser hides, none the renderer refused),
//! their meshes the renderer's ([`silico_render::render::hits`]: each face the ray meets, each
//! edge within 6 points, each vertex within 7, nearest the eye first): every kind, whatever
//! the selection's filters, as reaching what the filters or a nearer face keep from the
//! pointer is its point (the part 3 design's decision 2). Nothing is cast on meshes of another
//! model than the selection's (a model published, its meshes not read yet): the menu says
//! so. Each row is named as the element map names it ([`crate::viewport::ViewBody::name_of`]):
//! its element ("Face6"), its kind, the feature that made it, and "· hidden" where a face
//! hides it (`hits`' rule). Each body's boxes are made off the UI thread as the view takes
//! its mesh (`crate::boxes`), kept while the view holds it (the tab hidden, the body hidden
//! too), and let go the frame it doesn't. A right click before the boxes of a body it would
//! cast on are made waits for them, its menu opened once they are, a frame or more later, as
//! it would have opened at the click (the part 3 design's section 2): it never casts without
//! a body that's drawn. Waiting, it's let go as an open menu is closed, and takes the keys
//! as one does (Esc lets it go; the rest do nothing).
//!
//! **The list.** Choosing *Select Other…* opens it beside the click, its first row lit. The
//! row lit is the selection's hover ([`Selection::hold_hover`]: its face or edge outlined, its
//! vertex's dot drawn); hovering a row lights it, and Tab and the down arrow (Shift+Tab and
//! the up arrow back) and the wheel step through the rows, round from the last to the first.
//! A click on a row, or Enter, chooses it as a click on the canvas would: it replaces the
//! selection, Ctrl (Cmd on macOS) toggling it, and while a dialog's box of names takes the
//! canvas's picks, it toggles it into the box, which says if it can't take it. In the context
//! menu the keys step through its items that are on, and Enter chooses the one lit: Create
//! Sketch's opens its planes beside it, which the pointer chooses from; any other item is the
//! tab's to do ([`CanvasAct`]).
//!
//! **The face read.** A context menu opened over a face (what a click there would select)
//! reads it for its type as the selection view reads one (`crate::measures`, `elements`, the
//! tab's), keyed by this menu ([`FaceRead`]), never waited on: once its answer says it's a
//! plane, Sketch on this face is among the menu's items ([`Menu::remake`]), the item lit
//! kept by what it is, the keys' light never taken by the item now under a resting pointer
//! ([`pointer_lights`]); in the list the read changes no row.
//!
//! **Closing.** Esc, a press anywhere off the menu (on the canvas it does nothing more: its
//! press, its drag, and its release are the menu's, as the prototype's click closes a menu
//! and selects nothing), another right click on the canvas (which opens another), the
//! published model or a body's mesh changing, a modal or the shortcut sheet coming up over
//! it, a sketch opening, or the tab not shown, closes it; a plane chosen from Create Sketch's
//! planes closes it too (a press on them is the menu's, not off it). The canvas's right
//! clicks and left presses are taken in the order they came, so a right click and then a
//! press in one frame leave it closed. While it's open the pane's wheel doesn't zoom
//! ([`View::wheel_held`]), the pointer's hover isn't asked, the menu holds the keyboard's
//! focus, and no other shortcut acts (`crate::app`'s keys).
//!
//! Its lines are drawn as every menu's (`crate::menu`), in egui's menu frame (`menu::frame`);
//! its foot behind [`paint_foot`], of the theme's tokens.

use std::sync::Arc;

use eframe::egui::{
    self, Area, EventFilter, FontId, Id, Key, Order, Pos2, Rect, Sense, Stroke, Ui, UiBuilder,
    Vec2, WidgetInfo, WidgetType,
};
use silico_core::command::Revision;
use silico_core::command::meshes::MeshElement;
use silico_core::document::FeatureId;
use silico_core::naming::Name;
use silico_protocol::Mesh;
use silico_render::render::{Hit, MAX_HITS, Pickable, face_hit, hits};

use crate::boxes::{Boxes, Build};
use crate::canvas_menu::{
    CanvasAct, CanvasItem, CanvasMenu, Context, Planar, UNCAST, UNREAD, Under,
};
use crate::icons::Icon;
use crate::measures::{Measured, Of};
use crate::menu;
use crate::renderer::Element;
use crate::selection::{Cast, How, Selection, centre_of, square};
use crate::theme;
use crate::viewport::{LeftEvent, LeftKind, SecondaryClick, View, ViewFrame};

/// The list's heading, as the prototype has it (drawn in capitals).
pub const HEADING: &str = "Under the pointer, nearest first";

/// A key the menu takes while it's open, from `crate::app`'s keys (before the pane draws).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// Tab, or the down arrow: the next row.
    Next,
    /// Shift+Tab, or the up arrow: the row before.
    Back,
    /// Enter: the row lit chosen, Ctrl toggling it.
    Choose { ctrl: bool },
    /// Esc.
    Close,
}

/// The menu's keys this frame, taken from egui's input in the order pressed, so no other
/// shortcut, widget, or field sees them (the prototype's keydown for menus): Tab and the down
/// arrow step on, Shift+Tab and the up arrow back, Enter chooses (Ctrl, Cmd on macOS,
/// toggling), Esc closes.
pub fn keys(ctx: &egui::Context) -> Vec<MenuKey> {
    ctx.input_mut(|input| {
        let mut keys = Vec::new();
        input.events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            let taken = match key {
                Key::Tab if modifiers.shift => Some(MenuKey::Back),
                Key::Tab | Key::ArrowDown => Some(MenuKey::Next),
                Key::ArrowUp => Some(MenuKey::Back),
                Key::Enter => Some(MenuKey::Choose {
                    ctrl: modifiers.command,
                }),
                Key::Escape => Some(MenuKey::Close),
                _ => None,
            };
            match taken {
                Some(taken) => {
                    keys.push(taken);
                    false
                }
                None => true,
            }
        });
        keys
    })
}

/// Which of its two menus is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The canvas's context menu (`crate::canvas_menu`).
    Context,
    /// The list of what's under the pointer.
    List,
}

/// A row of the list: what's under the pointer, by its body and element, its name and
/// whether that's stable, whether a face hides it, and its words: the element ("Face6") and
/// its note ("Face · Block · hidden").
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub body: u32,
    pub element: Element,
    pub name: Arc<Name>,
    pub stable: bool,
    pub hidden: bool,
    pub label: String,
    pub note: String,
}

impl Row {
    /// What a screen reader (and a test) finds it by: its words and its note.
    pub fn said(&self) -> String {
        format!("{} {}", self.label, self.note)
    }
}

/// What the tab knows that the canvas's menus say ([`SelectOther::overlay`]): a feature's
/// name by its ID; why Edit can't open a feature's dialog now, if it can't (the feature
/// menus' reason, `crate::feature_menu::off`); and why sketch mode can't open now, if it
/// can't (`crate::feature_menu::sketch_off`).
pub(crate) struct Facts<'a> {
    pub(crate) names: &'a dyn Fn(&FeatureId) -> Option<String>,
    pub(crate) edit_off: &'a dyn Fn(&FeatureId) -> Option<String>,
    pub(crate) sketch_off: Option<String>,
}

/// The canvas's Select Other: its menu, while one is open, or the right click waiting for its
/// bodies' boxes to open one; each body's boxes, while the view holds its mesh; the keys
/// taken for the menu and not yet applied; whether a left press closed it (its stream, to
/// its end, is the menu's); and the egui pass it was last given a frame in.
#[derive(Debug, Default)]
pub struct SelectOther {
    menu: Option<Menu>,
    waiting: Option<Waiting>,
    boxes: Boxes,
    keys: Vec<MenuKey>,
    swallowing: bool,
    shown: Option<u64>,
    /// How many context menus have opened: each one's face read ([`FaceRead`]) is keyed by
    /// it, so no answer of another menu's is taken for it.
    opened: u64,
}

/// A right click whose menu waits for the boxes of the bodies it casts on: what it's opened
/// with once they're made, as it would have been at the click. The click; its frame (its
/// events let go); the bodies that frame drew, each with its mesh; what the pointer was over
/// then (none while a menu held the hover); the selection's model then; and whether the
/// bodies were of it (only then is a cast made).
#[derive(Debug)]
struct Waiting {
    click: SecondaryClick,
    frame: ViewFrame,
    meshes: Vec<(u32, Arc<Mesh>)>,
    hovered: Option<(u32, Element, Arc<Name>)>,
    model: Option<(u64, Revision)>,
    current: bool,
}

/// An open menu: which; where its right click was, in points; the context menu, its items
/// for what was under the pointer, and what they were made of (what's under the pointer,
/// and what the menu says of the rest), to make them again when its face's type is read;
/// the face under the pointer, read for its type; the layer Create Sketch's planes are on
/// while they're open, and whether they're held open (Enter chose Create Sketch, till the
/// keys light another item or the pointer moves onto anything in the menu but Create
/// Sketch); whether it's yet to be drawn (its first frame lets go of any planes a menu
/// before it held open); what the pointer was last on ([`Target`]); the rows the cast
/// found (none if it wasn't cast); the item or row lit and the one the pointer was last
/// over; what it was cast on (the selection's model, and each body drawn with its mesh);
/// the wheel's steps not yet whole; the list's scroll, whether it was just stepped, and its
/// width.
#[derive(Debug)]
struct Menu {
    phase: Phase,
    at: Pos2,
    context: CanvasMenu,
    under: Option<Under>,
    context_facts: Context,
    face: Option<FaceRead>,
    planes_layer: Option<egui::LayerId>,
    planes_open: bool,
    fresh: bool,
    target: Target,
    rows: Vec<Row>,
    lit: Option<usize>,
    pointed: Option<usize>,
    model: Option<(u64, Revision)>,
    meshes: Vec<(u32, Arc<Mesh>)>,
    wheel: f32,
    scroll: f32,
    stepped: bool,
    width: Option<f32>,
}

/// The face a context menu opened over, read for its type as the selection view reads a
/// face it shows (`crate::measures`, `elements`), so Sketch on this face is there only for a
/// plane: what the read is of (this menu, and the selection's model), what it reads (the
/// body the face is in, and the name the selection holds it by), the name a store writes
/// for it or why none can be, and whether its answer has been taken.
#[derive(Debug)]
struct FaceRead {
    of: Of,
    measured: Measured,
    stored: Result<Arc<Name>, String>,
    typed: bool,
}

/// What the person did in the menu this frame.
enum Act {
    /// The context menu's item at this place chosen.
    Context(usize),
    /// A plane chosen from Create Sketch's planes.
    Plane(silico_core::document::OriginPlane),
    /// A row chosen, Ctrl held or not.
    Choose(usize, bool),
}

/// What a frame gives the menu: the canvas's frame, whether its bodies are of the
/// selection's model (`current`: only then is a cast made), the tab's facts, and egui's
/// context.
#[derive(Clone, Copy)]
struct Given<'a> {
    frame: &'a ViewFrame,
    current: bool,
    facts: &'a Facts<'a>,
    ctx: &'a egui::Context,
}

/// One of the canvas's events this frame, in the order they came.
enum CanvasEvent<'a> {
    Left(&'a LeftEvent),
    Right(&'a SecondaryClick, bool),
}

impl SelectOther {
    /// Whether a menu is open.
    pub fn is_open(&self) -> bool {
        self.menu.is_some()
    }

    /// Which menu is open, if one is.
    pub fn phase(&self) -> Option<Phase> {
        self.menu.as_ref().map(|menu| menu.phase)
    }

    /// The rows the open menu's cast found, nearest the eye first (none with no menu open).
    pub fn rows(&self) -> &[Row] {
        self.menu.as_ref().map_or(&[], |menu| &menu.rows)
    }

    /// The open context menu, its items for what was under the pointer (kept while the list
    /// is open).
    pub fn context(&self) -> Option<&CanvasMenu> {
        self.menu.as_ref().map(|menu| &menu.context)
    }

    /// The item or row lit, if one is.
    pub fn lit(&self) -> Option<usize> {
        self.menu.as_ref().and_then(|menu| menu.lit)
    }

    /// The face the open context menu is over, read for its type
    /// ([`face_typed`](Self::face_typed)): whether its answer has been taken (or is still to
    /// come); none if the menu isn't over a face the selection's model finds.
    pub fn face_read(&self) -> Option<bool> {
        self.menu.as_ref()?.face.as_ref().map(|face| face.typed)
    }

    /// Whether a right click waits for its bodies' boxes to open its menu.
    pub fn is_waiting(&self) -> bool {
        self.waiting.is_some()
    }

    /// Whether it takes the window's keys: a menu open, or a right click waiting to open one.
    pub fn holds_keys(&self) -> bool {
        self.menu.is_some() || self.waiting.is_some()
    }

    /// The meshes whose boxes are kept for the next cast, by their bodies' numbers: each the
    /// view's, never one it has let go.
    pub fn kept_meshes(&self) -> Vec<&Arc<Mesh>> {
        self.boxes.meshes().collect()
    }

    /// The face under `pixel`'s centre in `frame`: of the bodies it drew (`view`'s meshes of
    /// them), the one the ray meets first ([`face_hit`]), cast on their boxes. The
    /// selection's where an edge's stroke covers its pick and the filter takes faces but not
    /// edges ([`Selection::overlay`]). [`Cast::Empty`] where no face is there, or where no box
    /// can be made; [`Cast::Waiting`] while a body the frame drew has its boxes still to come
    /// (asked for as the view takes its mesh, and a frame asked for as they're made): it
    /// never casts without a body that's drawn.
    pub(crate) fn face_under(&self, view: &View, frame: &ViewFrame, pixel: [u32; 2]) -> Cast {
        let mut bodies: Vec<(u32, &Pickable)> = Vec::new();
        for n in &frame.drawn {
            let Some(body) = view.bodies().get(n) else {
                continue;
            };
            match self.boxes.made_of(*n, &body.mesh) {
                Some(pickable) => bodies.push((*n, pickable)),
                None if self.boxes.failed() => return Cast::Empty,
                None => return Cast::Waiting,
            }
        }
        let pointer = pixel.map(|c| f64::from(c) + 0.5);
        match face_hit(
            &frame.camera,
            frame.size_px,
            frame.pixels_per_point,
            &bodies,
            pointer,
        ) {
            Some(hit) => Cast::Face(hit.body, hit.element),
            None => Cast::Empty,
        }
    }

    /// Its boxes made by `build` from now on (a test's, to see where and when it runs):
    /// those made before let go.
    pub(crate) fn building(&mut self, build: Build) {
        self.boxes = Boxes::new(build);
    }

    /// `key`, taken for the menu by the window's keys, applied when the tab's overlay is
    /// next given a frame (this one's, which comes after the keys).
    pub(crate) fn key(&mut self, key: MenuKey) {
        self.keys.push(key);
    }

    /// The read of the face the open context menu is over, while its type isn't taken yet:
    /// what it's of, and what's measured (the tab reads it, as the selection view's face is
    /// read: `crate::measures`).
    pub(crate) fn face_wanted(&self) -> Option<(Of, Measured)> {
        let face = self.menu.as_ref()?.face.as_ref()?;
        (!face.typed).then(|| (face.of, face.measured.clone()))
    }

    /// The open context menu's face read, for the read `of`: a plane, with its outward unit
    /// normal (`normal`, which `elements` gives a planar face alone), or not (none). If it's a
    /// plane, Sketch on this face and Look At are among its items from now on, as the
    /// selection view's button is once `elements` says so; the item lit stays lit. An answer
    /// of another menu's read is dropped.
    pub(crate) fn face_typed(&mut self, of: Of, normal: Option<[f64; 3]>, ctx: &egui::Context) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let Some(face) = menu
            .face
            .as_mut()
            .filter(|face| face.of == of && !face.typed)
        else {
            return;
        };
        face.typed = true;
        tracing::debug!(plane = normal.is_some(), "canvas_menu_face_typed");
        let Some(normal) = normal else {
            return;
        };
        let stored = face.stored.clone();
        let Some(under) = menu.under.as_mut() else {
            return;
        };
        under.plane = Some(Planar { stored, normal });
        menu.remake(ctx);
        ctx.request_repaint();
    }

    /// The canvas's right clicks and left button this frame (`frame`, given to the tab's
    /// overlay before the selection takes its own), for the menu, in the order they came: a
    /// right click opens it, cast (if `current`) on the bodies `frame` drew (`view`'s bodies
    /// of the model `selection` is read against), and a left press closes it; the keys taken
    /// for it applied, the wheel stepping its list, and it drawn over the canvas with
    /// `painter`'s context; it closed as the module says. `facts` give a feature's name, by
    /// its ID, for the rows, and Edit's reason it's off. The left events the selection and
    /// the box select are to be given (every one but those of a press that closed the menu),
    /// and what an item chosen asks of the tab.
    pub(crate) fn overlay(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        frame: &ViewFrame,
        painter: &egui::Painter,
        current: bool,
        facts: &Facts<'_>,
    ) -> (Vec<LeftEvent>, Option<CanvasAct>) {
        let ctx = painter.ctx().clone();
        let before = self.state();
        let pass = ctx.cumulative_pass_nr();
        let shown_last = self
            .shown
            .is_some_and(|last| last.saturating_add(1) >= pass);
        self.shown = Some(pass);
        // A tab not shown (its drag's end given it with a painter drawing nothing), or one
        // that wasn't last pass: its menu is let go, not drawn where it isn't. Its boxes are
        // kept: the view holds its meshes still.
        if !painter.is_visible() || !shown_last {
            self.close(selection, view, &ctx);
            self.keys.clear();
            if !painter.is_visible() {
                // Its left events still ended as the menu's, its right clicks not taken.
                let unshown = ViewFrame {
                    secondary: Vec::new(),
                    ..frame.clone()
                };
                let given = Given {
                    frame: &unshown,
                    current: false,
                    facts,
                    ctx: &ctx,
                };
                return (self.canvas_events(selection, view, &given).0, None);
            }
        }
        // Each body's boxes asked for as the view takes its mesh, made off the UI thread, and
        // those of a mesh it no longer holds let go: nothing keeps an old mesh.
        self.boxes
            .hold(view.bodies().iter().map(|(n, body)| (*n, &body.mesh)), &ctx);
        let menu_changed = self
            .menu
            .as_ref()
            .is_some_and(|menu| !still(menu.model, &menu.meshes, selection, view, frame));
        let waiting_changed = self
            .waiting
            .as_ref()
            .is_some_and(|waiting| !still(waiting.model, &waiting.meshes, selection, view, frame));
        if menu_changed {
            tracing::debug!("the model or a mesh changed under Select Other: closed");
            self.close(selection, view, &ctx);
        }
        if waiting_changed {
            tracing::debug!("the model or a mesh changed under Select Other's click: let go");
            self.waiting = None;
        }
        let given = Given {
            frame,
            current,
            facts,
            ctx: &ctx,
        };
        let (kept, clicked) = self.canvas_events(selection, view, &given);
        // A menu open (or waiting) since before this frame, closed by a press anywhere off
        // it; one this frame's right click opened is the canvas's events' to close.
        if !clicked {
            self.pressed_off(selection, view, &ctx);
        }
        // A right click waiting for its bodies' boxes, opened once they're made.
        self.open_waiting(selection, view, facts, &ctx);
        let mut asked = None;
        for key in std::mem::take(&mut self.keys) {
            // Waiting, Esc lets it go; the rest are dropped, nothing lit to step or choose.
            if self.menu.is_none() && key == MenuKey::Close && self.waiting.take().is_some() {
                tracing::debug!("Esc let go of Select Other's click");
                continue;
            }
            if let Some(act) = self.apply(key, selection, view, &ctx) {
                asked = Some(act);
            }
        }
        self.wheel(&ctx, frame);
        match self.show(&ctx) {
            Some(Act::Context(at)) => {
                if let Some(act) = self.context_chosen(at, selection, view, &ctx) {
                    asked = Some(act);
                }
            }
            Some(Act::Plane(plane)) => {
                tracing::debug!(?plane, "canvas_menu_plane_chosen");
                self.close(selection, view, &ctx);
                asked = Some(CanvasAct::CreateSketch(plane));
            }
            Some(Act::Choose(at, ctrl)) => self.choose(at, ctrl, selection, view, &ctx),
            None => {}
        }
        if let Some(menu) = &self.menu {
            let row = menu.lit.and_then(|at| menu.rows.get(at));
            selection.hold_hover(
                row.filter(|_| menu.phase == Phase::List)
                    .map(|row| (row.body, row.element, Arc::clone(&row.name), row.stable)),
            );
        }
        if self.state() != before {
            // The hover it lit (or let go) is drawn by the next frame, which an idle egui
            // draws only when asked.
            ctx.request_repaint();
        }
        view.wheel_held = self.menu.is_some();
        (kept, asked)
    }

    /// Which menu is open, and its item or row lit: what a frame changing asks the next to
    /// show.
    fn state(&self) -> Option<(Phase, Option<usize>)> {
        self.menu.as_ref().map(|menu| (menu.phase, menu.lit))
    }

    /// The frame's left events and right clicks, in the order they came (their `seq`): a
    /// right click opens the menu, or waits to (the frame's last; one before it, another,
    /// closes it), a left press while it's open or waiting closes it, and that press's stream
    /// (its moves, its release or cancel, and the double-click straight after its release)
    /// is the menu's, whether its boxes were made in time or not. The left events but those,
    /// and whether a right click opened the menu or waits to.
    fn canvas_events(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        given: &Given<'_>,
    ) -> (Vec<LeftEvent>, bool) {
        let frame = given.frame;
        let last = frame.secondary.len().saturating_sub(1);
        let mut events: Vec<(u32, CanvasEvent<'_>)> = frame
            .left
            .iter()
            .map(|event| (event.seq, CanvasEvent::Left(event)))
            .chain(
                frame
                    .secondary
                    .iter()
                    .enumerate()
                    .map(|(at, click)| (click.seq, CanvasEvent::Right(click, at == last))),
            )
            .collect();
        events.sort_by_key(|(seq, _)| *seq);
        let mut kept = Vec::with_capacity(frame.left.len());
        let mut ended = false;
        let mut opened = false;
        for (_, event) in events {
            let event = match event {
                CanvasEvent::Right(click, true) => {
                    self.clicked(click, selection, view, given);
                    opened = true;
                    continue;
                }
                CanvasEvent::Right(_, false) => {
                    self.close(selection, view, given.ctx);
                    continue;
                }
                CanvasEvent::Left(event) => event,
            };
            if ended && event.kind == LeftKind::DoubleClick {
                ended = false;
                continue;
            }
            ended = false;
            if self.swallowing {
                if matches!(event.kind, LeftKind::Release | LeftKind::Cancel) {
                    self.swallowing = false;
                    ended = event.kind == LeftKind::Release;
                }
                continue;
            }
            if self.holds_keys() && event.kind == LeftKind::Press {
                tracing::debug!("a press on the canvas closed Select Other");
                self.close(selection, view, given.ctx);
                // Closed after a right click this frame: no press is to close it again.
                opened = false;
                self.swallowing = true;
                continue;
            }
            kept.push(*event);
        }
        (kept, opened)
    }

    /// The menu closed by a press of any button off it, anywhere in the window: where egui's
    /// own hit test (the layer at the press) isn't the menu's, as it decides whether the canvas
    /// under it takes the press, so a press is the menu's or another's, never both or neither.
    fn pressed_off(&mut self, selection: &mut Selection, view: &mut View, ctx: &egui::Context) {
        if !self.holds_keys() {
            return;
        }
        // Waiting, no menu is drawn: a press anywhere is off it. Create Sketch's planes, open
        // beside it, are its own.
        let layer = self.menu.as_ref().map(Menu::layer);
        let planes_layer = self.menu.as_ref().and_then(|menu| menu.planes_layer);
        let presses: Vec<Pos2> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::PointerButton {
                        pos, pressed: true, ..
                    } => Some(*pos),
                    _ => None,
                })
                .collect()
        });
        if presses.iter().any(|at| {
            let on = ctx.layer_id_at(*at);
            layer.is_none() || (on != layer && (planes_layer.is_none() || on != planes_layer))
        }) {
            tracing::debug!("a press off Select Other closed it");
            self.close(selection, view, ctx);
        }
    }

    /// A right click at `click`: the context menu opened at once if it casts nothing (the
    /// frame's bodies aren't the selection's model's) or every body the frame drew has its
    /// boxes made (or none can be); else the click waits for them, its menu opened once they
    /// are ([`open_waiting`](Self::open_waiting)), as it would have been now.
    fn clicked(
        &mut self,
        click: &SecondaryClick,
        selection: &mut Selection,
        view: &mut View,
        given: &Given<'_>,
    ) {
        // The hover a menu held, if one was open, is its row's, not the pointer's.
        let held = selection.hover_held();
        let hovered = selection
            .hovered()
            .filter(|_| !held)
            .map(|hovered| (hovered.body, hovered.element, Arc::clone(&hovered.name)));
        self.close(selection, view, given.ctx);
        let meshes: Vec<(u32, Arc<Mesh>)> = given
            .frame
            .drawn
            .iter()
            .filter_map(|n| {
                view.bodies()
                    .get(n)
                    .map(|body| (*n, Arc::clone(&body.mesh)))
            })
            .collect();
        let waiting = Waiting {
            click: *click,
            frame: ViewFrame {
                left: Vec::new(),
                secondary: Vec::new(),
                ..given.frame.clone()
            },
            meshes,
            hovered,
            model: selection.model_of(),
            current: given.current,
        };
        if waiting.current && !self.boxes.failed() && !self.made(&waiting.meshes) {
            tracing::debug!(bodies = waiting.meshes.len(), "select_other_waits");
            self.waiting = Some(waiting);
        } else {
            self.open(waiting, selection, view, given.facts, given.ctx);
        }
    }

    /// Whether every one of `meshes` has its boxes made.
    fn made(&self, meshes: &[(u32, Arc<Mesh>)]) -> bool {
        meshes
            .iter()
            .all(|(n, mesh)| self.boxes.made_of(*n, mesh).is_some())
    }

    /// The right click waiting, its menu opened if its bodies' boxes are made now, or none
    /// can be.
    fn open_waiting(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        facts: &Facts<'_>,
        ctx: &egui::Context,
    ) {
        let Some(waiting) = self.waiting.take() else {
            return;
        };
        if self.boxes.failed() || self.made(&waiting.meshes) {
            tracing::debug!("select_other_waited");
            self.open(waiting, selection, view, facts, ctx);
        } else {
            self.waiting = Some(waiting);
        }
    }

    /// The context menu opened for the right click `at` waited for, cast (if its frame's
    /// bodies are current) on what that frame drew, over what a click there would select.
    fn open(
        &mut self,
        at: Waiting,
        selection: &mut Selection,
        view: &mut View,
        facts: &Facts<'_>,
        ctx: &egui::Context,
    ) {
        let Waiting {
            click,
            frame,
            meshes,
            hovered,
            model,
            current,
        } = at;
        let _span = tracing::debug_span!("select_other_open", current).entered();
        let ppp = frame.pixels_per_point;
        let at = frame.rect.min + Vec2::new(click.at_px[0], click.at_px[1]) / ppp;
        let cast = if current {
            self.cast(&click, view, &frame, &meshes, facts.names)
        } else {
            None
        };
        let (rows, count) = match cast {
            Some((rows, found)) => {
                let count = count_said(rows.len(), found);
                (rows, count)
            }
            None if current => (Vec::new(), UNCAST.to_owned()),
            None => (Vec::new(), UNREAD.to_owned()),
        };
        tracing::debug!(rows = rows.len(), "select_other_cast");
        // What a click there would select: the pointer's hover; or, while a menu held it,
        // the first of the cast's no face hides that the filters take.
        let filter = selection.picks();
        let pointed = hovered.or_else(|| {
            rows.iter()
                .find(|row| {
                    !row.hidden
                        && match row.element {
                            Element::Face(_) => filter.faces,
                            Element::Edge(_) => filter.edges,
                            Element::Vertex(_) => filter.vertices,
                        }
                })
                .map(|row| (row.body, row.element, Arc::clone(&row.name)))
        });
        let under = pointed
            .as_ref()
            .and_then(|(body, element, name)| under_of(view, facts, *body, *element, name));
        self.opened = self.opened.wrapping_add(1);
        // A face, read for its type as the selection view's is: found in the selection's
        // model as a pick is, for its body and the name a store writes for it.
        let face = match (&pointed, &under, selection.model_of()) {
            (Some((_, Element::Face(_), name)), Some(_), Some(model)) => selection
                .resolve_one(&name.to_string())
                .map(|found| FaceRead {
                    of: Of {
                        version: self.opened,
                        model,
                    },
                    measured: Measured::One {
                        body: found.feature.clone(),
                        name: Arc::clone(&found.name),
                    },
                    stored: found
                        .stored
                        .as_ref()
                        .map(Arc::clone)
                        .map_err(|why| why.message().to_owned()),
                    typed: false,
                }),
            _ => None,
        };
        let context = Context {
            count,
            listable: !rows.is_empty(),
            edit_off: under
                .as_ref()
                .and_then(|under| (facts.edit_off)(&under.maker)),
            selected: !selection.is_empty(),
            sketch_off: facts.sketch_off.clone(),
        };
        self.menu = Some(Menu {
            phase: Phase::Context,
            at: if at.x.is_finite() && at.y.is_finite() {
                at
            } else {
                frame.rect.center()
            },
            context: CanvasMenu::new(under.as_ref(), &context, ctx),
            under,
            context_facts: context,
            face,
            planes_layer: None,
            planes_open: false,
            fresh: true,
            target: Target::Off,
            rows,
            lit: None,
            pointed: None,
            model,
            meshes,
            wheel: 0.0,
            scroll: 0.0,
            stepped: false,
            width: None,
        });
        selection.hold_hover(None);
    }

    /// What's under `click`'s pixel in `frame`, of `meshes` (the bodies it drew, by number),
    /// as rows, and how many the cast found (some of which may not be rows: one its body's
    /// names don't name); none if a body's boxes aren't made, as no cast leaves one out.
    fn cast(
        &self,
        click: &SecondaryClick,
        view: &View,
        frame: &ViewFrame,
        meshes: &[(u32, Arc<Mesh>)],
        names: &dyn Fn(&FeatureId) -> Option<String>,
    ) -> Option<(Vec<Row>, usize)> {
        let bodies: Vec<(u32, &Pickable)> = meshes
            .iter()
            .map(|(n, mesh)| self.boxes.made_of(*n, mesh).map(|pickable| (*n, pickable)))
            .collect::<Option<_>>()?;
        let pixel = centre_of(square(click.at_px, frame.size_px, 0));
        let pointer = pixel.map(|c| f64::from(c) + 0.5);
        let found = hits(
            &frame.camera,
            frame.size_px,
            frame.pixels_per_point,
            &bodies,
            pointer,
        );
        let named = |hit: &Hit| view.bodies().get(&hit.body)?.name_of(hit.element);
        Some((rows_of(&found, named, names), found.len()))
    }

    /// `key` applied to the open menu: what an item it chose asks of the tab.
    fn apply(
        &mut self,
        key: MenuKey,
        selection: &mut Selection,
        view: &mut View,
        ctx: &egui::Context,
    ) -> Option<CanvasAct> {
        let menu = self.menu.as_mut()?;
        match key {
            MenuKey::Close => self.close(selection, view, ctx),
            MenuKey::Next => menu.step(1),
            MenuKey::Back => menu.step(-1),
            MenuKey::Choose { ctrl } => match (menu.phase, menu.lit) {
                (Phase::Context, Some(at)) => return self.context_chosen(at, selection, view, ctx),
                (Phase::List, Some(at)) => self.choose(at, ctrl, selection, view, ctx),
                (_, None) => {}
            },
        }
        None
    }

    /// The context menu's item `at` chosen, if it's on: *Select Other…* opens the list;
    /// Create Sketch opens its planes beside it, the menu staying open for one of them to be
    /// chosen; another closes the menu and is the tab's to do.
    fn context_chosen(
        &mut self,
        at: usize,
        selection: &mut Selection,
        view: &mut View,
        ctx: &egui::Context,
    ) -> Option<CanvasAct> {
        let menu = self.menu.as_mut()?;
        if !menu.context.on(at) {
            return None;
        }
        let act = match menu.context.items.get(at)?.item.clone() {
            CanvasItem::SelectOther => {
                menu.list();
                return None;
            }
            // Its planes opened beside it, a plane to be chosen there.
            CanvasItem::CreateSketch => {
                menu.planes_open = true;
                return None;
            }
            // Off with no stored name (the store's refusal its reason): never chosen.
            CanvasItem::SketchOnFace(stored) => CanvasAct::SketchOnFace(stored?),
            CanvasItem::Act(act) => act,
        };
        tracing::debug!(?act, "canvas_menu_chosen");
        self.close(selection, view, ctx);
        Some(act)
    }

    /// The wheel this frame, over the canvas or the menu, stepping the list: a line a row
    /// (a page, too; points by the lines they make), down to the next.
    fn wheel(&mut self, ctx: &egui::Context, frame: &ViewFrame) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        if menu.phase != Phase::List {
            return;
        }
        let over_menu = pointer_over(ctx, menu.layer());
        if frame.pointer_px.is_none() && !over_menu {
            return;
        }
        let line = ctx.options(|options| options.input_options.line_scroll_speed);
        let lines: f32 = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Point if line > 0.0 => delta.y / line,
                        egui::MouseWheelUnit::Point => 0.0,
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => delta.y,
                    }),
                    _ => None,
                })
                .sum()
        });
        let steps = wheel_steps(&mut menu.wheel, lines, menu.rows.len());
        if steps != 0 {
            menu.step(steps);
        }
    }

    /// The row `at` chosen, as a click on the canvas: the selection replaced, or with `ctrl`
    /// the row toggled; while a dialog's box takes the canvas's picks, toggled into the box.
    fn choose(
        &mut self,
        at: usize,
        ctrl: bool,
        selection: &mut Selection,
        view: &mut View,
        ctx: &egui::Context,
    ) {
        let Some(row) = self
            .menu
            .as_ref()
            .and_then(|menu| menu.rows.get(at))
            .cloned()
        else {
            return;
        };
        self.close(selection, view, ctx);
        let how = selection.canvas_how(if ctrl { How::Toggle } else { How::Replace });
        tracing::debug!(element = ?row.element, ?how, "select_other_chosen");
        selection.pick(&row.name.to_string(), how);
    }

    /// The menu closed, or the right click waiting to open one let go: the hover and the
    /// wheel let go, and the keyboard's focus. The tab's, as a modal or the shortcut sheet
    /// comes up over it, and each frame sketch mode holds the canvas (`Tab::overlay`).
    pub(crate) fn close(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        ctx: &egui::Context,
    ) {
        self.waiting = None;
        if self.menu.take().is_some() {
            selection.release_hover();
            view.wheel_held = false;
            ctx.memory_mut(|memory| memory.surrender_focus(menu_id()));
        }
    }

    /// The open menu drawn over the canvas, holding the keyboard's focus: what was done in
    /// it.
    fn show(&mut self, ctx: &egui::Context) -> Option<Act> {
        let menu = self.menu.as_mut()?;
        let at = match menu.phase {
            Phase::Context => menu.at + Vec2::splat(theme::CONTEXT_MENU_GAP),
            Phase::List => {
                let [x, y] = theme::SELECT_OTHER_OFFSET;
                menu.at + Vec2::new(x, y)
            }
        };
        let area = menu.layer().id;
        let mut act = None;
        Area::new(area)
            .order(Order::Foreground)
            .fixed_pos(at)
            .constrain(true)
            .show(ctx, |ui| {
                menu::frame(ui).show(ui, |ui| {
                    ui.scope_builder(UiBuilder::new().id_salt("select_other"), |ui| {
                        // Its rows stacked, as the prototype's are: each its own padding.
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let label = match menu.phase {
                            Phase::Context => "Canvas",
                            Phase::List => "Select Other",
                        };
                        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                            node.set_role(egui::accesskit::Role::Menu);
                            node.set_label(label);
                        });
                        // The keyboard's focus held by the menu, so Tab steps it and no
                        // widget behind it takes Tab, Enter, or Esc.
                        let held = ui.interact(
                            Rect::from_min_size(ui.min_rect().min, Vec2::ZERO),
                            menu_id(),
                            Sense::focusable_noninteractive(),
                        );
                        if !held.has_focus() {
                            held.request_focus();
                        }
                        ui.memory_mut(|memory| {
                            memory.set_focus_lock_filter(
                                menu_id(),
                                EventFilter {
                                    tab: true,
                                    horizontal_arrows: true,
                                    vertical_arrows: true,
                                    escape: true,
                                },
                            );
                        });
                        match menu.phase {
                            Phase::Context => context_ui(ui, menu, &mut act),
                            Phase::List => list_ui(ui, menu, &mut act),
                        }
                    });
                });
            });
        act
    }
}

/// Whether what a menu was cast on (or is to be), the selection's `model` and the bodies
/// drawn with their `meshes`, is still what `frame` draws: the selection's model, and each
/// body drawn with the same mesh.
fn still(
    model: Option<(u64, Revision)>,
    meshes: &[(u32, Arc<Mesh>)],
    selection: &Selection,
    view: &View,
    frame: &ViewFrame,
) -> bool {
    model == selection.model_of()
        && meshes.len() == frame.drawn.len()
        && meshes.iter().zip(&frame.drawn).all(|((n, mesh), drawn)| {
            n == drawn
                && view
                    .bodies()
                    .get(n)
                    .is_some_and(|body| Arc::ptr_eq(&body.mesh, mesh))
        })
}

impl Menu {
    /// The layer it's drawn on: each of its two menus an area of its own.
    fn layer(&self) -> egui::LayerId {
        let which = match self.phase {
            Phase::Context => "context",
            Phase::List => "list",
        };
        egui::LayerId::new(Order::Foreground, menu_id().with(which))
    }

    /// Its context menu made again from what it's made of (what's under the pointer, and
    /// [`Menu::context_facts`]), as when its face's read answers: the one place it's made,
    /// but at its opening. In the context menu the item lit is kept by what it is, not by its
    /// place, which an item put before it moves; the item under a resting pointer is taken
    /// for the pointer's the next frame, quietly ([`pointer_lights`]). In the list, `lit` is
    /// a row's place, and stays.
    fn remake(&mut self, ctx: &egui::Context) {
        let lit = self
            .lit
            .and_then(|at| self.context.items.get(at))
            .map(|shown| shown.item.clone());
        self.context = CanvasMenu::new(self.under.as_ref(), &self.context_facts, ctx);
        if self.phase == Phase::Context {
            self.lit = lit.and_then(|item| {
                self.context
                    .items
                    .iter()
                    .position(|shown| shown.item == item)
            });
        }
    }

    /// Whether its context menu's *Select Other…* can be chosen: something is under the
    /// pointer.
    fn listable(&self) -> bool {
        !self.rows.is_empty()
    }

    /// The list opened, its first row lit.
    fn list(&mut self) {
        if self.phase == Phase::Context && self.listable() {
            tracing::debug!(rows = self.rows.len(), "select_other_list");
            self.phase = Phase::List;
            self.lit = Some(0);
            self.pointed = None;
            self.scroll = 0.0;
            self.stepped = true;
        }
    }

    /// The lit row moved `by` rows, round the list's ends; in the context menu, the lit item
    /// moved `by` among those that are on.
    fn step(&mut self, by: i64) {
        if self.phase == Phase::Context {
            let before = self.lit;
            self.lit = self.context.step(self.lit, by);
            // The keys onto another item let Create Sketch's planes go.
            if self.lit != before {
                self.planes_open = false;
            }
            return;
        }
        let Ok(rows) = i64::try_from(self.rows.len()) else {
            return;
        };
        if rows == 0 || by == 0 {
            return;
        }
        let from = match self.lit.and_then(|at| i64::try_from(at).ok()) {
            Some(at) => at,
            // None lit: the first step forward lights the first, back the last.
            None if by > 0 => -1,
            None => rows,
        };
        // Taken round the list first, so no step overflows.
        let to = (from + by.rem_euclid(rows)).rem_euclid(rows);
        self.lit = usize::try_from(to).ok();
        self.stepped = true;
    }
}

/// The rows `lines` of the wheel step (rolled up, above 0, the content moving down: back),
/// with what `wheel` held of the last frames' that made no whole row, which it keeps of
/// these: at most a round of the list's `rows` each way, and none for a wheel that isn't
/// finite (which lets go of what it held).
fn wheel_steps(wheel: &mut f32, lines: f32, rows: usize) -> i64 {
    *wheel += lines;
    if !wheel.is_finite() {
        *wheel = 0.0;
        return 0;
    }
    let whole = wheel.trunc();
    *wheel -= whole;
    let rows = i64::try_from(rows).unwrap_or(i64::MAX).max(1);
    // A float's `as` saturates: a wheel past i64 is a round of the list, at most.
    -(whole as i64).clamp(-rows, rows)
}

/// The id the menu's focus and areas hang from.
fn menu_id() -> Id {
    Id::new("select_other_menu")
}

/// An element's kind, as the rows and the context menu's head say it, and its number.
fn kind_of(element: Element) -> (&'static str, u32) {
    match element {
        Element::Face(n) => ("Face", n),
        Element::Edge(n) => ("Edge", n),
        Element::Vertex(n) => ("Vertex", n),
    }
}

/// A feature's name, as `names` gives it, or its ID.
fn named(names: &dyn Fn(&FeatureId) -> Option<String>, id: &FeatureId) -> String {
    names(id).unwrap_or_else(|| id.as_str().to_owned())
}

/// What the context menu says is under the pointer: `element` of body `body`, by `name`;
/// none if its body isn't the view's.
fn under_of(
    view: &View,
    facts: &Facts<'_>,
    body: u32,
    element: Element,
    name: &Name,
) -> Option<Under> {
    let body = view.bodies().get(&body)?;
    let (kind, number) = kind_of(element);
    let maker = name.feature().clone();
    Some(Under {
        label: format!("{kind}{number}"),
        kind,
        maker_name: named(facts.names, &maker),
        maker,
        body_name: named(facts.names, &body.feature),
        body: body.feature.clone(),
        plane: None,
    })
}

/// What *Select Other…* says of a cast that found `found` and made `rows` rows of them: how
/// many rows, "or more" where the cast found its most ([`MAX_HITS`]), whatever rows it made
/// of them; "nothing" for no row.
fn count_said(rows: usize, found: usize) -> String {
    match rows {
        0 => "nothing under the pointer".to_owned(),
        n if found >= MAX_HITS => format!("{n} or more under the pointer"),
        n => format!("{n} under the pointer"),
    }
}

/// `found` as rows, nearest first, each named as `named` names it (its body's mesh's name of
/// its element), and the feature that made it by `names`: none for one `named` doesn't name.
fn rows_of<'a>(
    found: &[Hit],
    named: impl Fn(&Hit) -> Option<&'a MeshElement>,
    names: &dyn Fn(&FeatureId) -> Option<String>,
) -> Vec<Row> {
    found
        .iter()
        .filter_map(|hit| Some(row_of(hit, named(hit)?, names)))
        .collect()
}

/// `hit` as a row: named `named_as`, its note its kind, the feature that made it (by its
/// name, as `names` gives it, or its ID), and whether it's hidden.
fn row_of(hit: &Hit, named_as: &MeshElement, names: &dyn Fn(&FeatureId) -> Option<String>) -> Row {
    let (kind, number) = kind_of(hit.element);
    let made_by = named(names, named_as.name.feature());
    let mut note = format!("{kind} · {made_by}");
    if hit.hidden {
        note.push_str(" · hidden");
    }
    Row {
        body: hit.body,
        element: hit.element,
        name: Arc::clone(&named_as.name),
        stable: named_as.stable,
        hidden: hit.hidden,
        label: format!("{kind}{number}"),
        note,
    }
}

/// The context menu (`crate::canvas_menu`): the pointer onto an item lights it, and a click
/// chooses it.
fn context_ui(ui: &mut Ui, menu: &mut Menu, act: &mut Option<Act>) {
    let drawn = menu.context.draw(
        ui,
        menu.lit,
        menu.planes_open,
        std::mem::take(&mut menu.fresh),
    );
    let (pointed, hovered, clicked) = (drawn.pointed, drawn.hovered, drawn.clicked);
    menu.planes_layer = drawn.planes_layer;
    if let Some(plane) = drawn.plane {
        *act = Some(Act::Plane(plane));
        return;
    }
    if let Some(at) = pointer_lights(ui, pointed, &mut menu.pointed) {
        menu.lit = Some(at);
    }
    // Create Sketch's planes held open by Enter go once the pointer moves onto anything of
    // the menu but Create Sketch (another item, one that's off, the separator), as hovering
    // another item closes a submenu in every menu; a move into the planes, or off the menu,
    // keeps them. Onto: what the pointer is on changed ([`Target`]), so a nudge within what
    // it rests on, or a move of none, is no move onto it.
    let over_menu = pointer_over(ui.ctx(), ui.layer_id());
    let target = match hovered {
        Some(at) => Target::Item(at),
        None if over_menu => Target::Between,
        None => Target::Off,
    };
    let before = std::mem::replace(&mut menu.target, target);
    let on_create_sketch = pointed
        .and_then(|at| menu.context.items.get(at))
        .is_some_and(|shown| shown.item == CanvasItem::CreateSketch);
    let onto = target != before;
    if moved(ui) && over_menu && !on_create_sketch && onto {
        menu.planes_open = false;
    }
    if let Some(at) = clicked {
        *act = Some(Act::Context(at));
    }
}

/// What the context menu's pointer is on, as the last frame found it: off the menu (in
/// Create Sketch's planes, or past the menu); on it at no item (the separator, the padding);
/// or on the item at a place, whether that item is on or off. The pointer moves onto
/// something only when this changes: a move within it, or a move of none, isn't one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Off,
    Between,
    Item(usize),
}

/// Whether the pointer is over `layer`, read outside any input closure.
fn pointer_over(ctx: &egui::Context, layer: egui::LayerId) -> bool {
    ctx.pointer_hover_pos()
        .is_some_and(|at| ctx.layer_id_at(at) == Some(layer))
}

/// Whether the pointer moved this frame (a `PointerMoved` event came).
fn moved(ui: &Ui) -> bool {
    ui.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerMoved(_)))
    })
}

/// The one rule for the pointer in both menus, the context menu's items and the list's
/// rows: an item the pointer has moved onto lights (the prototype's mouseover, an event of
/// the pointer's moving), what's given back; one it rests on doesn't take the light from the
/// keys, though what lies under it changed (the list scrolled by the keys, or the context
/// menu made again when its face's read answered): that one is taken for the pointer's
/// (`last`), quietly.
fn pointer_lights(ui: &Ui, pointed: Option<usize>, last: &mut Option<usize>) -> Option<usize> {
    let lights = moved(ui) && pointed.is_some() && pointed != *last;
    *last = pointed;
    if lights { pointed } else { None }
}

/// A row's line, as every menu draws its items (`crate::menu`), lit or not.
fn row_line(row: &Row, lit: bool) -> menu::Item<'_> {
    menu::Item {
        icon: Some(icon_of(row.element)),
        label: &row.label,
        sub: Some(&row.note),
        caps: &[],
        off: None,
        danger: false,
        lit,
    }
}

/// The list: its heading, its rows (scrolling past [`theme::SELECT_OTHER_ROWS`], the lit row
/// kept in view as it's stepped to), and its foot.
fn list_ui(ui: &mut Ui, menu: &mut Menu, act: &mut Option<Act>) {
    // Its lines' widest, laid out once: they don't change while it's open.
    let width = *menu.width.get_or_insert_with(|| {
        let lines: Vec<menu::Item<'_>> = menu.rows.iter().map(|row| row_line(row, false)).collect();
        menu::width(ui, Some(HEADING), &lines, foot_width(ui))
    });
    menu::head(ui, HEADING, width);
    let row = theme::MENU_ITEM_HEIGHT;
    let shown = theme::SELECT_OTHER_ROWS as f32 * row;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("select_other_rows")
        .max_height(shown)
        .auto_shrink([true, true])
        .scroll_source(egui::scroll_area::ScrollSource {
            mouse_wheel: false,
            ..Default::default()
        });
    if menu.stepped
        && let Some(lit) = menu.lit
    {
        // The lit row brought into view: no farther than it must go.
        let top = lit as f32 * row;
        let offset = menu.scroll.max(top + row - shown).min(top).max(0.0);
        scroll = scroll.vertical_scroll_offset(offset);
        menu.stepped = false;
    }
    let ctrl = ui.input(|input| input.modifiers.command);
    let mut pointed = None;
    let output = scroll.show_rows(ui, row, menu.rows.len(), |ui, range| {
        for at in range {
            let Some(each) = menu.rows.get(at) else {
                continue;
            };
            let response = menu::item(ui, &row_line(each, menu.lit == Some(at)), width);
            if response.hovered() {
                pointed = Some(at);
            }
            if response.clicked() {
                *act = Some(Act::Choose(at, ctrl));
            }
        }
    });
    menu.scroll = output.state.offset.y;
    if let Some(at) = pointer_lights(ui, pointed, &mut menu.pointed) {
        menu.lit = Some(at);
    }
    paint_foot(ui, width);
}

/// A row's icon: its kind's (`selFace`, `selEdge`, `selVertex`).
fn icon_of(element: Element) -> Icon {
    match element {
        Element::Face(_) => Icon::SelFace,
        Element::Edge(_) => Icon::SelEdge,
        Element::Vertex(_) => Icon::SelVertex,
    }
}

/// The foot's parts: words, and a key cap's.
const FOOT: [(&str, bool); 6] = [
    ("Tab", true),
    (" or the wheel steps · ", false),
    ("Enter", true),
    (" selects · ", false),
    ("Esc", true),
    (" closes", false),
];

/// The width the foot's words and key caps take, padded.
fn foot_width(ui: &Ui) -> f32 {
    let padding = theme::MENU_FOOT_PADDING;
    let parts: f32 = FOOT
        .iter()
        .map(|(text, cap)| {
            if *cap {
                crate::start::key_caps_width(ui, &[(*text).to_owned()])
            } else {
                menu::text_width(ui, text, theme::MENU_FOOT_SIZE)
            }
        })
        .sum();
    f32::from(padding.left) + parts + f32::from(padding.right)
}

/// Select Other's foot (`.menu.select-other .foot`), `width` wide: under a rule, "Tab or the
/// wheel steps · Enter selects · Esc closes", its keys as key caps.
fn paint_foot(ui: &mut Ui, width: f32) {
    let padding = theme::MENU_FOOT_PADDING;
    let line = theme::KEY_CAP_HEIGHT;
    let height = theme::MENU_FOOT_MARGIN_TOP
        + theme::BORDER_WIDTH
        + f32::from(padding.top)
        + line
        + f32::from(padding.bottom);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let said: String = FOOT.iter().map(|(text, _)| *text).collect();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &said));
    let rule = rect.top() + theme::MENU_FOOT_MARGIN_TOP + theme::BORDER_WIDTH / 2.0;
    ui.painter().hline(
        rect.x_range(),
        rule,
        Stroke::new(theme::BORDER_WIDTH, theme::MENU_FOOT_RULE),
    );
    let middle = rule + theme::BORDER_WIDTH / 2.0 + f32::from(padding.top) + line / 2.0;
    let mut x = rect.left() + f32::from(padding.left);
    for (text, cap) in FOOT {
        if cap {
            let caps = [text.to_owned()];
            let wide = crate::start::key_caps_width(ui, &caps);
            crate::start::key_caps(ui, Pos2::new(x + wide, middle), &caps, false);
            x += wide;
        } else {
            let galley = ui.painter().layout_no_wrap(
                text.to_owned(),
                FontId::proportional(theme::MENU_FOOT_SIZE),
                theme::MENU_FOOT_TEXT,
            );
            let wide = galley.size().x;
            ui.painter().galley(
                Pos2::new(x, middle - galley.size().y / 2.0),
                galley,
                theme::MENU_FOOT_TEXT,
            );
            x += wide;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whats_under_a_stroke_waits_while_any_body_drawn_has_its_boxes_to_come() {
        use std::collections::BTreeMap;
        const WAIT: std::time::Duration = std::time::Duration::from_secs(60);
        // Body 1, a 2 by 1 by 1 box from the origin, over body 2, a unit box from
        // (0.5, 0, −1); seen from above, 4 mm high on 100 by 100 pixels, (x, y) lands at
        // (50 + 25 (x − 1), 50 − 25 (y − 0.5)): pixel (43, 50)'s centre is (0.74, 0.48), where
        // the ray meets body 1's top (z 1) first, then body 2's (z 0).
        let near = Arc::new(silico_render::bench::box_mesh([0.0; 3], [2.0, 1.0, 1.0]));
        let far = Arc::new(silico_render::bench::box_mesh([0.5, 0.0, -1.0], [1.0; 3]));
        let held = Arc::as_ptr(&near) as usize;
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = std::sync::Mutex::new(released);
        let build: Build = Arc::new(move |mesh: Arc<Mesh>| {
            if Arc::as_ptr(&mesh) as usize == held {
                released.lock().unwrap().recv_timeout(WAIT).unwrap();
            }
            Pickable::new(mesh)
        });
        let mut other = SelectOther::default();
        other.building(build);
        let ctx = egui::Context::default();
        let made = |other: &mut SelectOther, meshes: &[(u32, &Arc<Mesh>)], body: u32| {
            let deadline = std::time::Instant::now() + WAIT;
            loop {
                other.boxes.hold(meshes.iter().copied(), &ctx);
                let mesh = meshes
                    .iter()
                    .find(|(n, _)| *n == body)
                    .map(|(_, mesh)| *mesh);
                if mesh.is_some_and(|mesh| other.boxes.made_of(body, mesh).is_some()) {
                    return;
                }
                assert!(std::time::Instant::now() < deadline, "never made");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        // Body 2's boxes made, then body 1's asked for and held: one drawn body has its
        // boxes, the other not yet.
        made(&mut other, &[(2, &far)], 2);
        other.boxes.hold([(1, &near), (2, &far)].into_iter(), &ctx);
        assert!(other.boxes.made_of(1, &near).is_none());
        let body = |mesh: &Arc<Mesh>| crate::viewport::ViewBody {
            feature: FeatureId::new("k7f2qx3m").unwrap(),
            key: silico_core::recompute::Key::parse(&"01".repeat(32)).unwrap(),
            level: 0.1,
            deflection: 0.1,
            bounds: None,
            serves: [0.1, 0.2],
            held: false,
            mesh: Arc::clone(mesh),
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
        };
        let view = View::holding(BTreeMap::from([(1, body(&near)), (2, body(&far))]));
        let frame = ViewFrame {
            session: silico_core::command::SessionId::parse("s1").unwrap(),
            rect: Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0)),
            camera: silico_protocol::Camera {
                projection: silico_protocol::Projection::Orthographic { view_height: 4.0 },
                eye: [1.0, 0.5, 10.0],
                target: [1.0, 0.5, 0.0],
                up: [0.0, 1.0, 0.0],
            },
            size_px: [100, 100],
            pixels_per_point: 1.0,
            pointer_px: None,
            left: Vec::new(),
            secondary: Vec::new(),
            drawn: vec![1, 2],
            now: std::time::Instant::now(),
        };
        // Not known yet, though body 2's top is under it: a cast on body 2 alone would take
        // it, seen through body 1.
        assert_eq!(other.face_under(&view, &frame, [43, 50]), Cast::Waiting);
        // Body 1's made: its top.
        release.send(()).unwrap();
        made(&mut other, &[(1, &near), (2, &far)], 1);
        assert_eq!(
            other.face_under(&view, &frame, [43, 50]),
            Cast::Face(1, Element::Face(6))
        );
        // Body 1 not drawn (hidden): body 2's top.
        let hidden = ViewFrame {
            drawn: vec![2],
            ..frame
        };
        assert_eq!(
            other.face_under(&view, &hidden, [43, 50]),
            Cast::Face(2, Element::Face(6))
        );
    }

    fn menu(rows: usize, phase: Phase) -> Menu {
        let row = |n: u32| Row {
            body: 1,
            element: Element::Face(n),
            name: Arc::new(Name::Element {
                feature: FeatureId::new("k7f2qx3m").unwrap(),
                made: silico_core::naming::TypeWord::Primitive(
                    silico_core::naming::PrimitiveKind::Box,
                ),
                element: silico_protocol::ElementRef {
                    kind: silico_protocol::ElementKind::Face,
                    index: n,
                },
            }),
            stable: true,
            hidden: false,
            label: format!("Face{n}"),
            note: "Face · Block".to_owned(),
        };
        let ctx = egui::Context::default();
        let context = Context {
            count: format!("{rows} under the pointer"),
            listable: rows > 0,
            edit_off: None,
            selected: false,
            sketch_off: Some("Close the open dialog first".to_owned()),
        };
        // Over nothing, nothing selected, sketch mode unable to open: Fit and Home on, Clear
        // selection and Create Sketch off, and Select Other… on with rows.
        Menu {
            phase,
            at: Pos2::ZERO,
            context: CanvasMenu::new(None, &context, &ctx),
            under: None,
            context_facts: context,
            face: None,
            planes_layer: None,
            planes_open: false,
            fresh: true,
            target: Target::Off,
            rows: (1..=u32::try_from(rows).unwrap()).map(row).collect(),
            lit: None,
            pointed: None,
            model: None,
            meshes: Vec::new(),
            wheel: 0.0,
            scroll: 0.0,
            stepped: false,
            width: None,
        }
    }

    /// The block's top, Face6, under the pointer, read for its type: what the menu over it
    /// holds, its read `of`.
    fn over_the_top(of: Of, phase: Phase) -> Menu {
        let block = FeatureId::new("k7f2qx3m").unwrap();
        let under = Under {
            label: "Face6".to_owned(),
            kind: "Face",
            maker: block.clone(),
            maker_name: "Block".to_owned(),
            body: block.clone(),
            body_name: "Block".to_owned(),
            plane: None,
        };
        let base = menu(4, phase);
        let facts = Context {
            sketch_off: None,
            ..base.context_facts.clone()
        };
        let name = Arc::clone(&base.rows[0].name);
        Menu {
            context: CanvasMenu::new(Some(&under), &facts, &egui::Context::default()),
            under: Some(under),
            context_facts: facts,
            face: Some(FaceRead {
                of,
                measured: Measured::One {
                    body: block,
                    name: Arc::clone(&name),
                },
                stored: Ok(name),
                typed: false,
            }),
            ..base
        }
    }

    /// A planar face's unit normal with three distinct parts (0.48² + 0.6² + 0.64² = 1).
    const TILTED: [f64; 3] = [0.48, 0.6, 0.64];

    fn labels(so: &SelectOther) -> Vec<String> {
        so.context()
            .unwrap()
            .items
            .iter()
            .map(|shown| shown.label.clone())
            .collect()
    }

    #[test]
    fn a_face_read_as_a_plane_keeps_what_was_lit_in_the_menu_and_in_the_list() {
        // #444's review. Over the top: Select Other…, Edit Block, and Hide Block,
        // Hide lit by the keys. Another menu's answer is dropped; its own puts Sketch on this
        // face and Look At before Hide, and Hide stays lit, now fifth.
        let ctx = egui::Context::default();
        let of = Of {
            version: 7,
            model: (1, Revision::from_number(3).unwrap()),
        };
        let mut so = SelectOther {
            menu: Some(over_the_top(of, Phase::Context)),
            ..Default::default()
        };
        so.menu.as_mut().unwrap().lit = Some(2);
        let before = ["Select Other…", "Edit Block", "Hide Block"];
        so.face_typed(Of { version: 6, ..of }, Some(TILTED), &ctx);
        assert_eq!(labels(&so), before);
        assert_eq!((so.lit(), so.face_read()), (Some(2), Some(false)));
        so.face_typed(
            Of {
                model: (2, of.model.1),
                ..of
            },
            Some(TILTED),
            &ctx,
        );
        assert_eq!(labels(&so), before);
        so.face_typed(of, Some(TILTED), &ctx);
        assert_eq!(
            labels(&so),
            [
                "Select Other…",
                "Edit Block",
                "Sketch on this face",
                "Look At",
                "Hide Block"
            ]
        );
        assert_eq!((so.lit(), so.face_read()), (Some(4), Some(true)));
        assert_eq!(
            so.context().unwrap().items[3].item,
            CanvasItem::Act(CanvasAct::LookAt(TILTED))
        );
        // Edit lit instead, the pointer resting on it: Edit stays lit, second.
        let mut so = SelectOther {
            menu: Some(Menu {
                lit: Some(1),
                pointed: Some(1),
                ..over_the_top(of, Phase::Context)
            }),
            ..Default::default()
        };
        so.face_typed(of, Some(TILTED), &ctx);
        assert_eq!(so.lit(), Some(1));
        // In the list, row 2 lit and the pointer on it: a row's place, which the read
        // answering leaves alone.
        let mut list = SelectOther {
            menu: Some(Menu {
                lit: Some(2),
                pointed: Some(2),
                ..over_the_top(of, Phase::List)
            }),
            ..Default::default()
        };
        list.face_typed(of, Some(TILTED), &ctx);
        let menu = list.menu.as_ref().unwrap();
        assert_eq!(
            (menu.lit, menu.pointed, menu.phase),
            (Some(2), Some(2), Phase::List)
        );
        assert_eq!(list.face_read(), Some(true));
    }

    #[test]
    fn a_step_goes_round_the_lists_ends_and_from_none_lit_to_the_first_or_the_last() {
        let mut list = menu(4, Phase::List);
        list.step(1);
        assert_eq!(list.lit, Some(0));
        list.step(-1);
        assert_eq!(list.lit, Some(3));
        list.step(1);
        assert_eq!(list.lit, Some(0));
        list.step(6);
        assert_eq!(list.lit, Some(2));
        list.step(-7);
        assert_eq!(list.lit, Some(3));
        let mut from_none = menu(4, Phase::List);
        from_none.step(-1);
        assert_eq!(from_none.lit, Some(3));
        // Steps as large as an i64 go round, never overflowing.
        list.step(i64::MAX);
        list.step(i64::MIN);
        assert!(list.lit.is_some_and(|at| at < 4));
        // An empty list lights nothing; a context menu steps through its items that are on
        // (Fit, Home view, and Select Other… with rows; Clear selection off, as nothing is
        // selected, and Create Sketch, as sketch mode can't open): back from none to the last
        // on, Select Other…, and on round to Fit.
        let mut empty = menu(0, Phase::List);
        empty.step(1);
        assert_eq!(empty.lit, None);
        let mut context = menu(2, Phase::Context);
        context.step(-1);
        assert_eq!(context.lit, Some(2));
        context.step(1);
        assert_eq!(context.lit, Some(0));
        // With nothing under the pointer, Select Other… is off and passed over.
        let mut off = menu(0, Phase::Context);
        off.step(-1);
        assert_eq!(off.lit, Some(1));
        off.step(1);
        assert_eq!(off.lit, Some(0));
        // The list opens on its first row, and only from a context menu with rows.
        context.list();
        assert_eq!((context.phase, context.lit), (Phase::List, Some(0)));
        off.list();
        assert_eq!(off.phase, Phase::Context);
    }

    #[test]
    fn the_wheel_steps_a_row_a_line_keeps_its_fractions_and_survives_any_delta() {
        let mut held = 0.0;
        // Down (below 0) on, up back; a part of a line kept for the next.
        assert_eq!(wheel_steps(&mut held, -1.0, 5), 1);
        assert_eq!(wheel_steps(&mut held, 2.0, 5), -2);
        assert_eq!(wheel_steps(&mut held, -0.5, 5), 0);
        assert_eq!(wheel_steps(&mut held, -0.5, 5), 1);
        assert_eq!(held, 0.0);
        // Past the list a round of it at most, however far, and nothing that isn't finite.
        assert_eq!(wheel_steps(&mut held, -1e30, 5), 5);
        assert_eq!(wheel_steps(&mut held, f32::MAX, 5), -5);
        held = 0.25;
        assert_eq!(wheel_steps(&mut held, f32::NAN, 5), 0);
        assert_eq!(held, 0.0);
        assert_eq!(wheel_steps(&mut held, f32::INFINITY, 5), 0);
        assert_eq!(wheel_steps(&mut held, f32::NEG_INFINITY, 5), 0);
        assert_eq!(held, 0.0);
        assert_eq!(wheel_steps(&mut held, -3.0, 0), 1);
    }

    #[test]
    fn a_cast_at_its_most_says_or_more_by_what_it_found_whatever_rows_it_made_of_them() {
        // 256 found, the cast's most: Face1 to Face256 of body 1, nearest first, Face1 alone
        // unhidden; Face100's name not the view's, so 255 rows. "Or more" is of what the cast
        // found, not the rows: 255 rows of 256 found say "255 or more", where by the rows
        // alone they'd say 255, as if the cast had found all.
        let element = |n: u32| MeshElement {
            name: Arc::new(Name::Element {
                feature: FeatureId::new("k7f2qx3m").unwrap(),
                made: silico_core::naming::TypeWord::Primitive(
                    silico_core::naming::PrimitiveKind::Box,
                ),
                element: silico_protocol::ElementRef {
                    kind: silico_protocol::ElementKind::Face,
                    index: n,
                },
            }),
            stable: true,
            count: None,
        };
        let elements: Vec<MeshElement> = (1..=256).map(element).collect();
        let found: Vec<Hit> = (1..=256u32)
            .map(|n| Hit {
                body: 1,
                element: Element::Face(n),
                point: [0.0; 3],
                depth: f64::from(n),
                hidden: n > 1,
            })
            .collect();
        assert_eq!(found.len(), MAX_HITS);
        let named = |hit: &Hit| match hit.element {
            Element::Face(100) => None,
            Element::Face(n) => elements.get(usize::try_from(n).unwrap() - 1),
            Element::Edge(_) | Element::Vertex(_) => None,
        };
        let names = |_: &FeatureId| Some("Block".to_owned());
        let rows = rows_of(&found, named, &names);
        assert_eq!(rows.len(), 255);
        assert!(!rows.iter().any(|row| row.label == "Face100"));
        assert_eq!(rows[0].said(), "Face1 Face · Block");
        assert_eq!(rows[98].said(), "Face99 Face · Block · hidden");
        assert_eq!(rows[99].said(), "Face101 Face · Block · hidden");
        assert_eq!(
            count_said(rows.len(), found.len()),
            "255 or more under the pointer"
        );
        // At the most with every row; under it, with one dropped or none; and no row.
        assert_eq!(count_said(256, 256), "256 or more under the pointer");
        assert_eq!(count_said(3, 4), "3 under the pointer");
        assert_eq!(count_said(255, 255), "255 under the pointer");
        assert_eq!(count_said(0, 300), "nothing under the pointer");
    }

    #[test]
    fn the_menus_keys_are_taken_in_their_order_and_no_other_key_is() {
        let ctx = egui::Context::default();
        let key = |key: Key, modifiers: egui::Modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let input = egui::RawInput {
            events: vec![
                key(Key::Tab, egui::Modifiers::NONE),
                key(Key::A, egui::Modifiers::NONE),
                key(Key::Tab, egui::Modifiers::SHIFT),
                key(Key::ArrowDown, egui::Modifiers::NONE),
                key(Key::ArrowUp, egui::Modifiers::NONE),
                key(Key::Enter, egui::Modifiers::COMMAND),
                key(Key::Enter, egui::Modifiers::NONE),
                key(Key::Escape, egui::Modifiers::NONE),
            ],
            ..Default::default()
        };
        let mut taken = Vec::new();
        let mut left = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            taken = keys(ui.ctx());
            left = ui.input(|input| input.events.clone());
        });
        output.textures_delta.clear();
        assert_eq!(
            taken,
            [
                MenuKey::Next,
                MenuKey::Back,
                MenuKey::Next,
                MenuKey::Back,
                MenuKey::Choose { ctrl: true },
                MenuKey::Choose { ctrl: false },
                MenuKey::Close,
            ]
        );
        assert_eq!(left, [key(Key::A, egui::Modifiers::NONE)]);
    }

    // ---- Drawn, and given frames, as the tab gives them ----

    use crate::canvas_menu::SELECT_OTHER;
    use crate::viewport::{LeftEvent, LeftKind};
    use egui::Color32;

    /// The app's theme (its fonts, and every menu's style) on egui's context.
    fn themed() -> egui::Context {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx
    }

    /// A pass of `ctx` on a 1000-point square screen at one pixel a point, at `time` seconds
    /// (a second on from the last, so a new area's fade-in is over by the pass after it shows),
    /// with `events`; `run` given its root Ui. What it painted.
    fn pass(
        ctx: &egui::Context,
        time: u32,
        events: Vec<egui::Event>,
        run: impl FnMut(&mut Ui),
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1000.0))),
            time: Some(f64::from(time)),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, run);
        output.textures_delta.clear();
        output
    }

    /// Whether a repaint was asked for in the pass that gave `output`.
    fn repaint_asked(output: &egui::FullOutput) -> bool {
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|viewport| viewport.repaint_delay.is_zero())
    }

    /// The shapes `output` painted, in order, each of a group in its place.
    fn shapes(output: egui::FullOutput) -> Vec<egui::Shape> {
        fn flat(shape: egui::Shape, into: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        flat(shape, into);
                    }
                }
                shape => into.push(shape),
            }
        }
        let mut all = Vec::new();
        for clipped in output.shapes {
            flat(clipped.shape, &mut all);
        }
        all
    }

    /// A menu opened at (300, 300) with `rows` rows.
    fn opened(rows: usize, phase: Phase) -> Menu {
        Menu {
            at: Pos2::new(300.0, 300.0),
            ..menu(rows, phase)
        }
    }

    /// A left event of `kind`, `seq`th in its frame.
    fn left(kind: LeftKind, seq: u32) -> LeftEvent {
        LeftEvent {
            kind,
            at_px: [10.0, 10.0],
            shift: false,
            ctrl: false,
            frame: 0,
            time: 0.0,
            seq,
        }
    }

    /// A right click at `at_px`, `seq`th in its frame.
    fn right(at_px: [f32; 2], seq: u32) -> SecondaryClick {
        SecondaryClick {
            at_px,
            shift: false,
            ctrl: false,
            frame: 0,
            time: 0.0,
            seq,
        }
    }

    /// A canvas 400 by 300 points from (100, 50), at two pixels a point, given `left` and
    /// `secondary`; the pointer over it, at `pointer_px`.
    fn canvas(
        left: Vec<LeftEvent>,
        secondary: Vec<SecondaryClick>,
        pointer_px: Option<[f32; 2]>,
    ) -> ViewFrame {
        ViewFrame {
            session: silico_core::command::SessionId::parse("s1").unwrap(),
            rect: Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(400.0, 300.0)),
            camera: crate::navigate::HOME.camera(crate::navigate::NO_BOX, 800, 600),
            size_px: [800, 600],
            pixels_per_point: 2.0,
            pointer_px,
            left,
            secondary,
            drawn: Vec::new(),
            now: std::time::Instant::now(),
        }
    }

    fn wheeled(unit: egui::MouseWheelUnit, lines: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit,
            delta: Vec2::new(0.0, lines),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn colour(hex: u32) -> Color32 {
        let [_, r, g, b] = hex.to_be_bytes();
        Color32::from_rgb(r, g, b)
    }

    fn near(a: Pos2, b: Pos2) -> bool {
        (a - b).length() < 1e-3
    }

    /// A text shape's words, its first section's size and colour, and where it's drawn.
    fn text_of(shape: &egui::Shape) -> (String, f32, Color32, Pos2, Vec2) {
        let egui::Shape::Text(text) = shape else {
            panic!("not text: {shape:?}");
        };
        let format = &text.galley.job.sections[0].format;
        (
            text.galley.text().to_owned(),
            format.font_id.size,
            format.color,
            text.pos,
            text.galley.size(),
        )
    }

    #[test]
    fn the_foot_is_the_prototypes_its_words_and_key_caps_under_a_rule() {
        // styles.css, `.menu.select-other .foot`: `margin-top: 4px`, `border-top: 1px solid
        // var(--g250)` (#272626), `padding: 4px 8px 2px`, `font-size: 10.5px`, `color:
        // var(--g550)` (#939090); interact.js's `openSelectOther`, its words "Tab or the
        // wheel steps · Enter selects · Esc closes", the keys `.kbd`s (`height: 16px;
        // min-width: 16px; padding: 0 4px; border: 1px solid var(--g350)` (#525151),
        // `border-radius: 3px; background: var(--g250); color: var(--g800)` (#e0dfdf),
        // `font-size: 10.5px`, all border-box). Put at (10, 20), by hand: the rule 4 down,
        // its 1 point's middle at 24.5; its line 4 under it, 16 high, from 29 to 45 (its
        // middle 37, a cap's words' line under its 1-point top border, their middle 38); 2
        // under that, 27 high in all; its words from 8 in, a cap as wide as its words and
        // its padding and borders (10), or 16, and the foot 8 wider again at its right.
        let ctx = themed();
        let parts = [
            ("Tab", true),
            (" or the wheel steps · ", false),
            ("Enter", true),
            (" selects · ", false),
            ("Esc", true),
            (" closes", false),
        ];
        let mut measured = Vec::new();
        let mut drawn = None;
        let output = pass(&ctx, 1, Vec::new(), |ui| {
            measured = parts
                .iter()
                .map(|(text, _)| {
                    ui.painter()
                        .layout_no_wrap(
                            (*text).to_owned(),
                            FontId::proportional(10.5),
                            Color32::WHITE,
                        )
                        .size()
                })
                .collect();
            let place = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(900.0, 100.0));
            let inner = ui.scope_builder(UiBuilder::new().max_rect(place), |ui| {
                let width = foot_width(ui);
                paint_foot(ui, width);
                width
            });
            drawn = Some((inner.inner, inner.response.rect));
        });
        let (width, rect) = drawn.unwrap();
        let wide = |(at, (_, cap)): (usize, &(&str, bool))| {
            let text = measured[at].x;
            if *cap { (text + 10.0).max(16.0) } else { text }
        };
        let words: f32 = parts.iter().enumerate().map(wide).sum();
        assert!((width - (8.0 + words + 8.0)).abs() < 1e-3, "{width}");
        assert!(near(rect.min, Pos2::new(10.0, 20.0)), "{rect:?}");
        assert!((rect.width() - width).abs() < 1e-3, "{rect:?}");
        assert!((rect.height() - 27.0).abs() < 1e-3, "{rect:?}");
        let shapes = shapes(output);
        // The rule, then each cap (its fill, its border, its words) and each run of words.
        assert_eq!(shapes.len(), 13, "{shapes:#?}");
        let egui::Shape::LineSegment { points, stroke } = &shapes[0] else {
            panic!("no rule: {:?}", shapes[0]);
        };
        assert!(near(points[0], Pos2::new(10.0, 24.5)), "{points:?}");
        assert!(near(points[1], Pos2::new(10.0 + width, 24.5)), "{points:?}");
        assert_eq!(*stroke, Stroke::new(1.0, colour(0x272626)));
        let mut x = 18.0;
        let mut at = 1;
        for (n, (text, cap)) in parts.iter().enumerate() {
            let size = measured[n];
            if *cap {
                let cap_wide = (size.x + 10.0).max(16.0);
                let egui::Shape::Rect(fill) = &shapes[at] else {
                    panic!("no cap for {text}: {:?}", shapes[at]);
                };
                let place = Rect::from_min_max(Pos2::new(x, 29.0), Pos2::new(x + cap_wide, 45.0));
                assert!(
                    near(fill.rect.min, place.min) && near(fill.rect.max, place.max),
                    "{text}: {fill:?}"
                );
                assert_eq!(fill.fill, colour(0x272626), "{text}");
                assert_eq!(fill.corner_radius, egui::CornerRadius::same(3), "{text}");
                let egui::Shape::Mesh(border) = &shapes[at + 1] else {
                    panic!("no border for {text}: {:?}", shapes[at + 1]);
                };
                let bounds = border.calc_bounds();
                // On the pixels the cap's box is rounded to, feathered half a pixel out.
                let off = (bounds.min - place.min)
                    .abs()
                    .max((bounds.max - place.max).abs());
                assert!(off.x <= 1.0 && off.y <= 1.0, "{text}: {bounds:?} {place:?}");
                assert!(
                    border
                        .vertices
                        .iter()
                        .any(|vertex| vertex.color == colour(0x525151)),
                    "{text}"
                );
                let (said, font, ink, pos, galley) = text_of(&shapes[at + 2]);
                assert_eq!((said.as_str(), font, ink), (*text, 10.5, colour(0xe0dfdf)));
                let centre = Pos2::new(x + cap_wide / 2.0, 38.0);
                assert!(near(pos, centre - galley / 2.0), "{text}: {pos:?}");
                x += cap_wide;
                at += 3;
            } else {
                let (said, font, ink, pos, _) = text_of(&shapes[at]);
                assert_eq!((said.as_str(), font, ink), (*text, 10.5, colour(0x939090)));
                assert!(
                    near(pos, Pos2::new(x, 37.0 - size.y / 2.0)),
                    "{text}: {pos:?}"
                );
                x += size.x;
                at += 1;
            }
        }
        assert!((x + 8.0 - (10.0 + width)).abs() < 1e-3, "{x}");
    }

    #[test]
    fn its_menus_open_beside_the_click_framed_holding_the_focus_the_lit_row_alone_filled() {
        // The context menu 1 point right of and below its right click (so the click's own
        // point stays the canvas's), the list 28 right and 18 up (interact.js,
        // `openSelectOther`'s `e.clientX + 28, e.clientY - 18`); each framed as `.menu` is
        // (`background: var(--g200)`, #212121; `border: 1px solid var(--g250)`, #272626;
        // `border-radius: var(--r)`, 6), holding the keyboard's focus. The list's lit row
        // alone filled `--blue` (#186add): its first, under the frame's border and its 4 of
        // padding, and its head (6 + 16 + 2 high).
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(opened(4, Phase::Context)),
            ..Default::default()
        };
        let mut drawn = None;
        for time in 1..=3 {
            drawn = Some(pass(&ctx, time, Vec::new(), |ui| {
                assert!(so.show(ui.ctx()).is_none());
            }));
        }
        let layer = so.menu.as_ref().unwrap().layer();
        let area = ctx.memory(|memory| memory.area_rect(layer.id)).unwrap();
        assert_eq!(area.min, Pos2::new(301.0, 301.0));
        let framed = |shapes: &[egui::Shape], area: Rect| {
            let frame = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect) if rect.fill == colour(0x212121) => Some(rect),
                    _ => None,
                })
                .expect("no frame");
            assert!(
                near(frame.rect.min, area.min) && near(frame.rect.max, area.max),
                "{frame:?} {area:?}"
            );
            assert_eq!(frame.stroke, Stroke::new(1.0, colour(0x272626)));
            assert_eq!(frame.corner_radius, egui::CornerRadius::same(6));
        };
        framed(&shapes(drawn.take().unwrap()), area);
        assert!(ctx.memory(|memory| memory.has_focus(menu_id())));
        // The list, its first row lit.
        so.menu.as_mut().unwrap().list();
        for time in 4..=6 {
            drawn = Some(pass(&ctx, time, Vec::new(), |ui| {
                assert!(so.show(ui.ctx()).is_none());
            }));
        }
        let layer = so.menu.as_ref().unwrap().layer();
        let area = ctx.memory(|memory| memory.area_rect(layer.id)).unwrap();
        assert_eq!(area.min, Pos2::new(328.0, 282.0));
        let painted = shapes(drawn.take().unwrap());
        let blue: Vec<Rect> = painted
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == colour(0x186add) => Some(rect.rect),
                _ => None,
            })
            .collect();
        assert_eq!(blue.len(), 1, "{blue:?}");
        assert!(near(blue[0].min, Pos2::new(333.0, 311.0)), "{blue:?}");
        assert!((blue[0].height() - 24.0).abs() < 1e-3, "{blue:?}");
        framed(&painted, area);
        assert!(ctx.memory(|memory| memory.has_focus(menu_id())));
    }

    #[test]
    fn the_lit_row_is_scrolled_into_view_no_farther_than_it_must_go() {
        // 30 rows of 24, 16 shown (384 high, the list's 0 to 384 of its 720): the 18th
        // lit, from 408 to 432, scrolled to 432 − 384 = 48; the 15th (336 to 360) already
        // in view, left there; the 2nd (24 to 48) brought down to 24.
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(opened(30, Phase::Context)),
            ..Default::default()
        };
        so.menu.as_mut().unwrap().list();
        let scrolled = |so: &mut SelectOther, by: i64, time: u32| {
            so.menu.as_mut().unwrap().step(by);
            for time in time..time + 2 {
                pass(&ctx, time, Vec::new(), |ui| {
                    so.show(ui.ctx());
                });
            }
            let menu = so.menu.as_ref().unwrap();
            (menu.lit, menu.scroll)
        };
        assert_eq!(scrolled(&mut so, 0, 1), (Some(0), 0.0));
        assert_eq!(scrolled(&mut so, 17, 3), (Some(17), 48.0));
        assert_eq!(scrolled(&mut so, -3, 5), (Some(14), 48.0));
        assert_eq!(scrolled(&mut so, -13, 7), (Some(1), 24.0));
        // The wheel over the list steps it (the overlay's) and never scrolls it itself.
        let row = Pos2::new(340.0, 330.0);
        for time in 9..20 {
            let events = if time == 9 {
                vec![
                    egui::Event::PointerMoved(row),
                    wheeled(egui::MouseWheelUnit::Line, -3.0),
                ]
            } else {
                Vec::new()
            };
            pass(&ctx, time, events, |ui| {
                so.show(ui.ctx());
            });
        }
        let menu = so.menu.as_ref().unwrap();
        assert_eq!((menu.lit, menu.scroll), (Some(1), 24.0));
    }

    #[test]
    fn the_wheel_steps_the_list_over_the_canvas_or_over_the_list_and_nowhere_else() {
        use egui::MouseWheelUnit::{Line, Point};
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(opened(4, Phase::Context)),
            ..Default::default()
        };
        so.menu.as_mut().unwrap().list();
        for time in 1..=3 {
            pass(&ctx, time, Vec::new(), |ui| {
                so.show(ui.ctx());
            });
        }
        let list = ctx
            .memory(|memory| memory.area_rect(so.menu.as_ref().unwrap().layer().id))
            .unwrap();
        let away = Pos2::new(50.0, 50.0);
        assert!(!list.contains(away));
        let wheel = |so: &mut SelectOther, time: u32, at: Pos2, over: bool, unit, lines| {
            let pointer = over.then_some([10.0, 10.0]);
            let events = vec![egui::Event::PointerMoved(at), wheeled(unit, lines)];
            pass(&ctx, time, events, |ui| {
                so.wheel(ui.ctx(), &canvas(Vec::new(), Vec::new(), pointer));
                so.show(ui.ctx());
            });
            so.menu.as_ref().unwrap().lit
        };
        // Over the canvas (its pointer in its frame), down a line: on.
        assert_eq!(wheel(&mut so, 4, away, true, Line, -1.0), Some(1));
        // Over the list, which the canvas's frame has no pointer under: on.
        assert_eq!(wheel(&mut so, 5, list.center(), false, Line, -1.0), Some(2));
        // Over neither (another pane): nothing.
        assert_eq!(wheel(&mut so, 6, away, false, Line, -1.0), Some(2));
        // Points with no line to make them lines (a line of 0 points) step nothing, and keep
        // what the wheel held of a line: half a line, nothing of the points, half a line, one
        // step.
        ctx.options_mut(|options| options.input_options.line_scroll_speed = 0.0);
        assert_eq!(wheel(&mut so, 7, away, true, Line, -0.5), Some(2));
        assert_eq!(wheel(&mut so, 8, away, true, Point, -10.0), Some(2));
        assert_eq!(wheel(&mut so, 9, away, true, Line, -0.5), Some(3));
    }

    #[test]
    fn a_face_read_under_a_resting_pointer_leaves_the_keys_light_on_edit() {
        // #444's review. Over the top: its head, then Select Other…, Edit Block, a separator,
        // and Hide Block. The pointer moved onto Hide lights it; the keys step back to Edit,
        // and the pointer rests on Hide. The face's read answers (a plane): Sketch on this face
        // is put where Hide was, under the resting pointer. The keys' light stays on Edit:
        // only the pointer's moving onto an item takes it (egui's items are keyed by their
        // place, so the item under the pointer reads hovered at once).
        let ctx = themed();
        let of = Of {
            version: 7,
            model: (1, Revision::from_number(3).unwrap()),
        };
        let mut so = SelectOther {
            menu: Some(Menu {
                at: Pos2::new(300.0, 300.0),
                ..over_the_top(of, Phase::Context)
            }),
            ..Default::default()
        };
        // The menu at (301, 301), its lines from 5 in (its border and its padding): its head
        // 6 + 16 + 2 high, then 24 an item, the separator 1 + 2 × 4 before Hide.
        let first =
            306.0 + theme::MENU_HEAD_TOP + theme::MENU_LINE_HEIGHT + theme::MENU_HEAD_BOTTOM;
        let hide = Pos2::new(330.0, first + 2.0 * 24.0 + 9.0 + 12.0);
        let lit = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
            let events = at.map(egui::Event::PointerMoved).into_iter().collect();
            pass(&ctx, time, events, |ui| {
                so.show(ui.ctx());
            });
            so.menu.as_ref().unwrap().lit
        };
        assert_eq!(lit(&mut so, 1, None), None);
        assert_eq!(lit(&mut so, 2, None), None);
        assert_eq!(lit(&mut so, 3, Some(hide)), Some(2));
        so.menu.as_mut().unwrap().step(-1);
        assert_eq!(lit(&mut so, 4, None), Some(1));
        so.face_typed(of, Some(TILTED), &ctx);
        for time in 5..9 {
            assert_eq!(lit(&mut so, time, None), Some(1), "{time}");
        }
        // A point's nudge within the item it rests on, Sketch on this face, isn't moving onto
        // it: the keys' light stays.
        assert_eq!(lit(&mut so, 9, Some(hide + Vec2::new(1.0, 0.0))), Some(1));
        // The pointer then moved onto another item, Select Other…: lit; and back onto the one
        // now where Hide was, Sketch on this face: lit.
        let select_other = Pos2::new(330.0, first + 12.0);
        assert_eq!(lit(&mut so, 10, Some(select_other)), Some(0));
        assert_eq!(lit(&mut so, 11, Some(hide)), Some(2));
    }

    #[test]
    fn a_step_that_scrolls_the_list_under_a_resting_pointer_keeps_the_keys_light() {
        // #444's review: a list of 30, 16 shown; the pointer moved onto its fourth row (from
        // 5 in, below its head: 24 a row) lights it, then rests. The keys step down to the
        // 21st, scrolling the list under the pointer: the light is the keys', every frame.
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(opened(30, Phase::List)),
            ..Default::default()
        };
        // Beside the click at (300, 300), by its offset (28, −18): from (328, 282), its lines 5
        // in, below its head.
        let first =
            287.0 + theme::MENU_HEAD_TOP + theme::MENU_LINE_HEIGHT + theme::MENU_HEAD_BOTTOM;
        let fourth = Pos2::new(360.0, first + 3.0 * 24.0 + 12.0);
        let lit = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
            let events = at.map(egui::Event::PointerMoved).into_iter().collect();
            pass(&ctx, time, events, |ui| {
                so.show(ui.ctx());
            });
            so.menu.as_ref().unwrap().lit
        };
        lit(&mut so, 1, None);
        lit(&mut so, 2, None);
        assert_eq!(lit(&mut so, 3, Some(fourth)), Some(3));
        for want in 4..=20u32 {
            so.menu.as_mut().unwrap().step(1);
            let lit_row = Some(usize::try_from(want).unwrap());
            assert_eq!(lit(&mut so, 2 * want, None), lit_row);
            assert_eq!(lit(&mut so, 2 * want + 1, None), lit_row);
        }
        assert!(so.menu.as_ref().unwrap().scroll > 0.0);
        // A point's nudge within the row now under the resting pointer (another, scrolled
        // there) isn't moving onto it: the keys' light stays.
        assert_eq!(
            lit(&mut so, 50, Some(fourth + Vec2::new(1.0, 0.0))),
            Some(20)
        );
    }

    #[test]
    fn a_list_opened_under_a_resting_pointer_lights_its_first_row() {
        // The module's promise: Select Other… chosen, the list opens beside the click, its
        // first row lit, whatever row lands under the resting pointer.
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(opened(8, Phase::Context)),
            ..Default::default()
        };
        let lit = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
            let events = at.map(egui::Event::PointerMoved).into_iter().collect();
            pass(&ctx, time, events, |ui| {
                so.show(ui.ctx());
            });
            so.menu.as_ref().unwrap().lit
        };
        // The pointer moved over the menu, where the list's rows will be, then resting.
        lit(&mut so, 1, None);
        lit(&mut so, 2, Some(Pos2::new(360.0, 395.0)));
        so.menu.as_mut().unwrap().list();
        for time in 3..8 {
            assert_eq!(lit(&mut so, time, None), Some(0), "{time}");
        }
    }

    /// Over nothing, with Create Sketch on (sketch mode able to open): Fit, Home view,
    /// Select Other…, a separator, Clear selection (off), and Create Sketch, lit and its
    /// planes held open by Enter.
    fn planes_held(ctx: &egui::Context) -> Menu {
        let mut menu = opened(2, Phase::Context);
        menu.context_facts.sketch_off = None;
        menu.context = CanvasMenu::new(None, &menu.context_facts, ctx);
        menu.lit = Some(4);
        menu.planes_open = true;
        menu
    }

    #[test]
    fn the_pointer_onto_another_item_lets_the_planes_enter_held_open_go() {
        // #444's review: the menu at (301, 301), its lines from 5 in, 24 a row: Fit 306 to
        // 330, Home view to 354, Select Other… to 378, the separator to 387, Clear selection
        // (off) to 411, Create Sketch to 435. The planes Enter held open stay while the
        // pointer rests, or moves within Create Sketch, or into them; they go once it moves
        // onto anything else of the menu: Home view, the separator, Clear selection (off).
        for (other, what) in [
            (Pos2::new(330.0, 342.0), "Home view"),
            (Pos2::new(330.0, 382.5), "the separator"),
            (Pos2::new(330.0, 399.0), "Clear selection, off"),
        ] {
            let ctx = themed();
            let mut so = SelectOther {
                menu: Some(planes_held(&ctx)),
                ..Default::default()
            };
            let held = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
                let events = at.map(egui::Event::PointerMoved).into_iter().collect();
                pass(&ctx, time, events, |ui| {
                    so.show(ui.ctx());
                });
                so.menu.as_ref().unwrap().planes_open
            };
            assert!(held(&mut so, 1, None));
            assert!(held(&mut so, 2, None));
            assert!(held(&mut so, 3, Some(Pos2::new(330.0, 423.0))));
            assert!(held(&mut so, 4, Some(Pos2::new(340.0, 424.0))));
            // Into the planes, beside the menu: held still.
            let planes = so.menu.as_ref().unwrap().planes_layer.unwrap();
            let inside = ctx
                .memory(|memory| memory.area_rect(planes.id))
                .unwrap()
                .center();
            assert!(held(&mut so, 5, Some(inside)), "{what}");
            assert!(held(&mut so, 6, Some(Pos2::new(330.0, 423.0))), "{what}");
            assert!(!held(&mut so, 7, Some(other)), "{what}");
        }
    }

    #[test]
    fn a_nudge_within_the_item_the_pointer_rests_on_keeps_the_planes_enter_held_open() {
        // #444's review (round 4): the pointer resting on Fit (306 to 330), the keys back
        // round to Create Sketch, and Enter: a move of a point within Fit is no move onto
        // another item, as `pointer_lights` has it (Fit stays unlit), so the planes stay;
        // a move of none, as a window may send, too. One onto Home view lets them go.
        let ctx = themed();
        let mut menu = opened(2, Phase::Context);
        menu.context_facts.sketch_off = None;
        menu.context = CanvasMenu::new(None, &menu.context_facts, &ctx);
        let mut so = SelectOther {
            menu: Some(menu),
            ..Default::default()
        };
        let frame = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
            let events = at.map(egui::Event::PointerMoved).into_iter().collect();
            pass(&ctx, time, events, |ui| {
                so.show(ui.ctx());
            });
            let menu = so.menu.as_ref().unwrap();
            (menu.lit, menu.planes_open)
        };
        let fit = Pos2::new(330.0, 318.0);
        frame(&mut so, 1, None);
        frame(&mut so, 2, None);
        assert_eq!(frame(&mut so, 3, Some(fit)), (Some(0), false));
        let menu = so.menu.as_mut().unwrap();
        menu.step(-1);
        menu.planes_open = true;
        assert_eq!(frame(&mut so, 4, None), (Some(4), true));
        let nudged = fit + Vec2::new(1.0, 0.0);
        assert_eq!(frame(&mut so, 5, Some(nudged)), (Some(4), true));
        assert_eq!(frame(&mut so, 6, Some(nudged)), (Some(4), true));
        assert_eq!(
            frame(&mut so, 7, Some(Pos2::new(330.0, 342.0))),
            (Some(1), false)
        );
    }

    #[test]
    fn a_press_where_no_layer_is_closes_a_menu_whose_planes_arent_open() {
        // #444's mutants: with Create Sketch's planes not open, a press where egui finds no
        // layer at all is off the menu, and closes it; a press on the menu (Fit, 306 to 330)
        // doesn't. With the planes closed, `on` is None only off every layer, past the
        // screen's 1000 points here; the same rule with the planes open, a press on the
        // background, is the next test's.
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(opened(2, Phase::Context)),
            ..Default::default()
        };
        for time in 1..=2 {
            pass(&ctx, time, Vec::new(), |ui| {
                so.show(ui.ctx());
            });
        }
        assert_eq!(so.menu.as_ref().unwrap().planes_layer, None);
        let (mut selection, mut view) = (Selection::default(), View::default());
        let press = |at: Pos2| {
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let fit = Pos2::new(330.0, 318.0);
        pass(&ctx, 3, press(fit), |ui| {
            so.pressed_off(&mut selection, &mut view, ui.ctx());
        });
        assert!(so.is_open());
        let away = Pos2::new(1100.0, 1100.0);
        let mut found = None;
        pass(&ctx, 4, press(away), |ui| {
            found = Some(ui.ctx().layer_id_at(away));
            so.pressed_off(&mut selection, &mut view, ui.ctx());
        });
        assert_eq!(found, Some(None), "no layer at the press");
        assert!(!so.is_open());
    }

    #[test]
    fn a_press_on_the_background_with_the_planes_open_closes_the_menu() {
        // #444's review (round 5, T16): Create Sketch's planes held open by Enter and drawn,
        // two frames; a press at (50, 50), on the background's layer, neither the menu's
        // nor the planes': off both, so the menu closes, as a press on the browser, the
        // timeline, or the toolbar does.
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(planes_held(&ctx)),
            ..Default::default()
        };
        for time in 1..=2 {
            pass(&ctx, time, Vec::new(), |ui| {
                so.show(ui.ctx());
            });
        }
        assert!(so.menu.as_ref().unwrap().planes_layer.is_some());
        let (mut selection, mut view) = (Selection::default(), View::default());
        let at = Pos2::new(50.0, 50.0);
        let press = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        pass(&ctx, 3, press, |ui| {
            so.pressed_off(&mut selection, &mut view, ui.ctx());
        });
        assert!(!so.is_open());
    }

    #[test]
    fn from_off_the_menu_or_its_planes_onto_the_separator_or_an_off_item_lets_the_planes_go() {
        // #444's review (round 5, T15): the planes Enter held open; the pointer off the
        // menu, or in the planes; then straight onto the separator (378 to 387) or Clear
        // selection, off (387 to 411): onto something of the menu but Create Sketch, so the
        // planes go.
        for (onto, what) in [
            (Pos2::new(330.0, 382.5), "the separator"),
            (Pos2::new(330.0, 399.0), "Clear selection, off"),
        ] {
            for from_planes in [false, true] {
                let ctx = themed();
                let mut so = SelectOther {
                    menu: Some(planes_held(&ctx)),
                    ..Default::default()
                };
                let held = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
                    let events = at.map(egui::Event::PointerMoved).into_iter().collect();
                    pass(&ctx, time, events, |ui| {
                        so.show(ui.ctx());
                    });
                    so.menu.as_ref().unwrap().planes_open
                };
                assert!(held(&mut so, 1, None));
                assert!(held(&mut so, 2, None));
                let from = if from_planes {
                    let planes = so.menu.as_ref().unwrap().planes_layer.unwrap();
                    ctx.memory(|memory| memory.area_rect(planes.id))
                        .unwrap()
                        .center()
                } else {
                    Pos2::new(20.0, 20.0)
                };
                assert!(
                    held(&mut so, 3, Some(from)),
                    "{what}, from the planes: {from_planes}"
                );
                assert!(
                    !held(&mut so, 4, Some(onto)),
                    "{what}, from the planes: {from_planes}"
                );
            }
        }
    }

    #[test]
    fn a_move_of_none_over_an_off_item_keeps_the_planes_enter_held_open() {
        // #444's review (round 5, Q24): the pointer resting on Clear selection, off (387 to
        // 411), and the keys on Create Sketch, Enter holding its planes open; then a move to
        // the same point, as a window may send: no move onto anything, so they stay.
        let ctx = themed();
        let mut menu = planes_held(&ctx);
        menu.planes_open = false;
        let mut so = SelectOther {
            menu: Some(menu),
            ..Default::default()
        };
        let held = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
            let events = at.map(egui::Event::PointerMoved).into_iter().collect();
            pass(&ctx, time, events, |ui| {
                so.show(ui.ctx());
            });
            let menu = so.menu.as_ref().unwrap();
            (menu.lit, menu.planes_open)
        };
        let clear = Pos2::new(330.0, 399.0);
        held(&mut so, 1, None);
        held(&mut so, 2, None);
        assert_eq!(held(&mut so, 3, Some(clear)), (Some(4), false));
        // On the off item itself, not between items (#444's review, round 6, T18).
        assert_eq!(so.menu.as_ref().unwrap().target, Target::Item(3));
        so.menu.as_mut().unwrap().planes_open = true;
        assert_eq!(held(&mut so, 4, None), (Some(4), true));
        assert_eq!(held(&mut so, 5, Some(clear)), (Some(4), true));
        assert_eq!(held(&mut so, 6, Some(clear)), (Some(4), true));
    }

    #[test]
    fn a_move_from_an_off_item_onto_another_or_from_the_separator_onto_one_lets_the_planes_go() {
        // #444's review (round 6, T18): egui calls no disabled widget hovered, but an off item
        // is still what the pointer is on. Nothing under the pointer, so Select Other… is off
        // (354 to 378), as Clear selection is (387 to 411): the pointer resting on Select
        // Other… as Enter holds Create Sketch's planes open, a move of none keeps them, and one
        // onto Clear selection lets them go. Resting on the separator (378 to 387) instead, a
        // move onto Clear selection lets them go too.
        let clear = Pos2::new(330.0, 399.0);
        for (from, what) in [
            (Pos2::new(330.0, 366.0), "Select Other…, off"),
            (Pos2::new(330.0, 382.5), "the separator"),
        ] {
            // A context of its own for each case: its passes' times start again at 1.
            let ctx = themed();
            let mut menu = opened(0, Phase::Context);
            menu.context_facts.sketch_off = None;
            menu.context = CanvasMenu::new(None, &menu.context_facts, &ctx);
            menu.lit = Some(4);
            let held = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
                let events = at.map(egui::Event::PointerMoved).into_iter().collect();
                pass(&ctx, time, events, |ui| {
                    so.show(ui.ctx());
                });
                so.menu.as_ref().unwrap().planes_open
            };
            let mut so = SelectOther {
                menu: Some(menu),
                ..Default::default()
            };
            assert!(so.menu.as_ref().unwrap().context.items[2].off.is_some());
            held(&mut so, 1, None);
            held(&mut so, 2, None);
            assert!(!held(&mut so, 3, Some(from)));
            so.menu.as_mut().unwrap().planes_open = true;
            assert!(held(&mut so, 4, None), "{what}");
            assert!(held(&mut so, 5, Some(from)), "{what}: a move of none");
            assert!(
                !held(&mut so, 6, Some(clear)),
                "{what}: onto Clear selection"
            );
        }
    }

    #[test]
    fn a_thousand_menus_leave_egui_a_bounded_number_of_layers() {
        // #444's review: each right click opens a new menu, a frame each, a thousand times:
        // the menus share their layers (one for the context menu, one for the list), so
        // egui's layers stay few, not one more a menu.
        let ctx = egui::Context::default();
        let names = |_: &FeatureId| None;
        let edit_off = |_: &FeatureId| None;
        let facts = Facts {
            names: &names,
            edit_off: &edit_off,
            sketch_off: None,
        };
        let (mut selection, mut view) = (Selection::default(), View::default());
        let mut so = SelectOther::default();
        for time in 1..=1000u32 {
            let click = canvas(Vec::new(), vec![right([40.0, 30.0], time)], None);
            pass(&ctx, time, Vec::new(), |ui| {
                let painter = ui.painter().clone();
                so.overlay(&mut selection, &mut view, &click, &painter, false, &facts);
            });
            assert!(so.is_open());
        }
        let layers = ctx.memory(|memory| memory.layer_ids().count());
        assert!(layers < 16, "{layers} layers");
    }

    #[test]
    fn the_pointer_onto_an_item_lights_it_and_resting_there_or_leaving_it_keeps_the_keys_light() {
        // Over nothing, with rows: Fit, Home view, Select Other…, Clear selection (off). The
        // menu at (301, 301), its lines from 5 in (its border and its padding): Fit from 306
        // to 330, Home view below it.
        let ctx = themed();
        let mut so = SelectOther {
            menu: Some(opened(2, Phase::Context)),
            ..Default::default()
        };
        let lit = |so: &mut SelectOther, time: u32, at: Option<Pos2>| {
            let events = at.map(egui::Event::PointerMoved).into_iter().collect();
            pass(&ctx, time, events, |ui| {
                so.show(ui.ctx());
            });
            so.menu.as_ref().unwrap().lit
        };
        assert_eq!(lit(&mut so, 1, None), None);
        assert_eq!(lit(&mut so, 2, None), None);
        // Onto Fit: lit.
        assert_eq!(lit(&mut so, 3, Some(Pos2::new(330.0, 318.0))), Some(0));
        // The keys step to Home view, the pointer resting on Fit: Home view stays lit.
        so.menu.as_mut().unwrap().step(1);
        assert_eq!(lit(&mut so, 4, None), Some(1));
        assert_eq!(lit(&mut so, 5, None), Some(1));
        // Off the menu: what's lit stays.
        assert_eq!(lit(&mut so, 6, Some(Pos2::new(50.0, 50.0))), Some(1));
    }

    #[test]
    fn a_press_that_closed_the_menu_is_its_own_to_its_release_and_the_double_click_after() {
        use LeftKind::{Cancel, DoubleClick, Move, Press, Release};
        let ctx = egui::Context::default();
        let names = |_: &FeatureId| None;
        let edit_off = |_: &FeatureId| None;
        let facts = Facts {
            names: &names,
            edit_off: &edit_off,
            sketch_off: None,
        };
        let kept = |swallowing: bool, kinds: &[LeftKind]| -> (Vec<LeftKind>, bool) {
            let mut so = SelectOther {
                swallowing,
                ..Default::default()
            };
            let (mut selection, mut view) = (Selection::default(), View::default());
            let events = (0..)
                .zip(kinds)
                .map(|(seq, kind)| left(*kind, seq))
                .collect();
            let frame = canvas(events, Vec::new(), None);
            let given = Given {
                frame: &frame,
                current: false,
                facts: &facts,
                ctx: &ctx,
            };
            let (kept, opened) = so.canvas_events(&mut selection, &mut view, &given);
            assert!(!opened);
            (kept.iter().map(|event| event.kind).collect(), so.swallowing)
        };
        // Its release ends it, the double-click straight after that release is its too, and
        // the press after is the canvas's.
        assert_eq!(
            kept(true, &[Move, Release, DoubleClick, Press, Release]),
            (vec![Press, Release], false)
        );
        // A press straight after its release is the canvas's.
        assert_eq!(
            kept(true, &[Release, Press, Release]),
            (vec![Press, Release], false)
        );
        // A cancel makes no double-click: what comes straight after it is the canvas's.
        assert_eq!(
            kept(true, &[Cancel, DoubleClick]),
            (vec![DoubleClick], false)
        );
        // Not ended yet: still the menu's.
        assert_eq!(kept(true, &[Move]), (vec![], true));
        // With no press of the menu's, a double-click is the canvas's.
        assert_eq!(
            kept(false, &[DoubleClick, Press]),
            (vec![DoubleClick, Press], false)
        );
    }

    #[test]
    fn the_menu_opens_at_its_clicks_point_or_the_canvass_centre_for_one_not_finite() {
        let ctx = egui::Context::default();
        let names = |_: &FeatureId| None;
        let edit_off = |_: &FeatureId| None;
        let facts = Facts {
            names: &names,
            edit_off: &edit_off,
            sketch_off: None,
        };
        let at = |at_px: [f32; 2]| {
            let mut so = SelectOther::default();
            let (mut selection, mut view) = (Selection::default(), View::default());
            let waiting = Waiting {
                click: right(at_px, 0),
                frame: canvas(Vec::new(), Vec::new(), None),
                meshes: Vec::new(),
                hovered: None,
                model: None,
                current: false,
            };
            so.open(waiting, &mut selection, &mut view, &facts, &ctx);
            // Cast on nothing: the meshes aren't the selection's model's.
            let said = so.context().unwrap().items[2].said();
            assert_eq!(said, format!("{SELECT_OTHER} {UNREAD}"));
            so.menu.unwrap().at
        };
        // 40 and 30 pixels in, at 2 a point: 20 and 15 points from the canvas's corner.
        assert_eq!(at([40.0, 30.0]), Pos2::new(120.0, 65.0));
        // Either not finite: the canvas's centre, (300, 200).
        assert_eq!(at([f32::NAN, 30.0]), Pos2::new(300.0, 200.0));
        assert_eq!(at([40.0, f32::INFINITY]), Pos2::new(300.0, 200.0));
        assert_eq!(at([f32::NAN, f32::NEG_INFINITY]), Pos2::new(300.0, 200.0));
    }

    #[test]
    fn a_tab_not_shown_closes_its_menu_and_takes_no_right_click_and_one_shown_again_does() {
        let ctx = egui::Context::default();
        let names = |_: &FeatureId| None;
        let edit_off = |_: &FeatureId| None;
        let facts = Facts {
            names: &names,
            edit_off: &edit_off,
            sketch_off: None,
        };
        let (mut selection, mut view) = (Selection::default(), View::default());
        let mut so = SelectOther::default();
        let mut overlay = |so: &mut SelectOther, time: u32, shown: bool, frame: &ViewFrame| {
            let mut kept = Vec::new();
            pass(&ctx, time, Vec::new(), |ui| {
                let mut painter = ui.painter().clone();
                if !shown {
                    painter.set_invisible();
                }
                let (left, asked) =
                    so.overlay(&mut selection, &mut view, frame, &painter, false, &facts);
                assert_eq!(asked, None);
                kept = left.iter().map(|event| event.kind).collect::<Vec<_>>();
            });
            (kept, selection.hover_held())
        };
        // A pass the tab isn't shown in: its overlay not given it.
        let unshown = |time: u32| {
            pass(&ctx, time, Vec::new(), |_| {});
        };
        let nothing = canvas(Vec::new(), Vec::new(), None);
        let click = canvas(Vec::new(), vec![right([40.0, 30.0], 1)], None);
        // Shown, and shown the next pass: the list stays open, holding the hover.
        assert_eq!(overlay(&mut so, 1, true, &nothing), (vec![], false));
        so.menu = Some(opened(4, Phase::Context));
        so.menu.as_mut().unwrap().list();
        assert_eq!(overlay(&mut so, 2, true, &nothing), (vec![], true));
        assert_eq!(so.phase(), Some(Phase::List));
        // Not shown a pass, then shown: closed, the hover let go.
        unshown(3);
        assert!(so.is_open());
        assert_eq!(overlay(&mut so, 4, true, &nothing), (vec![], false));
        assert!(!so.is_open());
        // Not shown a pass, then shown with a right click: its menu opens.
        unshown(5);
        assert_eq!(overlay(&mut so, 6, true, &click), (vec![], true));
        assert_eq!(so.phase(), Some(Phase::Context));
        // A press closes it, its stream the menu's; the tab hidden mid-stream, given its
        // end unshown with the frame it was last shown in (a right click in it): the stream
        // ended, the menu not opened again.
        let press = canvas(vec![left(LeftKind::Press, 0)], Vec::new(), None);
        assert_eq!(overlay(&mut so, 7, true, &press), (vec![], false));
        assert!(!so.is_open() && so.swallowing);
        let ended = canvas(
            vec![left(LeftKind::Cancel, 0)],
            vec![right([40.0, 30.0], 1)],
            None,
        );
        assert_eq!(overlay(&mut so, 8, false, &ended), (vec![], false));
        assert!(!so.is_open() && !so.is_waiting() && !so.swallowing);
    }

    #[test]
    fn a_change_of_its_menu_or_its_light_asks_for_the_frame_that_draws_it() {
        let ctx = themed();
        let names = |_: &FeatureId| None;
        let edit_off = |_: &FeatureId| None;
        let facts = Facts {
            names: &names,
            edit_off: &edit_off,
            sketch_off: None,
        };
        let (mut selection, mut view) = (Selection::default(), View::default());
        let mut so = SelectOther::default();
        let nothing = canvas(Vec::new(), Vec::new(), None);
        let mut overlay = |so: &mut SelectOther, time: u32| {
            let output = pass(&ctx, time, Vec::new(), |ui| {
                let painter = ui.painter().clone();
                so.overlay(&mut selection, &mut view, &nothing, &painter, false, &facts);
            });
            repaint_asked(&output)
        };
        overlay(&mut so, 1);
        so.menu = Some(opened(4, Phase::Context));
        so.menu.as_mut().unwrap().list();
        // Its first frame (egui's sizing pass, which asks for the next) aside.
        overlay(&mut so, 2);
        overlay(&mut so, 3);
        assert!(!overlay(&mut so, 4), "a repaint asked with nothing changed");
        so.key(MenuKey::Next);
        assert!(
            overlay(&mut so, 5),
            "no repaint asked for the row stepped to"
        );
        assert_eq!(so.lit(), Some(1));
        // egui draws the pass after one a repaint is asked in too.
        overlay(&mut so, 6);
        assert!(!overlay(&mut so, 7), "a repaint asked with nothing changed");
        so.key(MenuKey::Close);
        assert!(overlay(&mut so, 8), "no repaint asked for the menu closed");
        assert!(!so.is_open());
    }
}
