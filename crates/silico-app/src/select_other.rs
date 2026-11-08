//! Select Other (M5.7; docs/design/m5.0-app.md, section 9's *Selection*): a right click on the
//! canvas opens a menu, *Select Other…*, and that a list of everything under the pointer,
//! nearest the eye first, as SolidWorks lists it (docs/ux/solidworks.md §1, *Select Other*)
//! and the approved prototype draws it (docs/design/m5-ui: DESIGN.md's *Menus*, interact.js's
//! `onContext` and `openSelectOther`).
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
//! hides it (`hits`' rule). Each body's boxes are made once for each mesh it's cast on.
//!
//! **The list.** Choosing *Select Other…* opens it beside the click, its first row lit. The
//! row lit is the selection's hover ([`Selection::hold_hover`]: its face or edge outlined, its
//! vertex's dot drawn); hovering a row lights it, and Tab and the down arrow (Shift+Tab and
//! the up arrow back) and the wheel step through the rows, round from the last to the first.
//! A click on a row, or Enter, chooses it as a click on the canvas would: it replaces the
//! selection, Ctrl (Cmd on macOS) toggling it, and while a dialog's box of names takes the
//! canvas's picks, it toggles it into the box, which says if it can't take it.
//!
//! **Closing.** Esc, a press anywhere off the menu (on the canvas it does nothing more: its
//! press, its drag, and its release are the menu's, as the prototype's click closes a menu
//! and selects nothing), another right click on the canvas (which opens another), the
//! published model or a body's mesh changing, or the tab not shown, closes it. While it's open
//! the pane's wheel doesn't zoom ([`View::wheel_held`]), the pointer's hover isn't asked, the
//! menu holds the keyboard's focus, and no other shortcut acts (`crate::app`'s keys).
//!
//! The visuals are behind small functions ([`menu_frame`], [`paint_row`], [`paint_head`],
//! [`paint_foot`]), each of the theme's tokens, which the UI port may replace.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{
    self, Area, EventFilter, FontId, Id, Key, Order, Pos2, Rect, Sense, Stroke, Ui, UiBuilder,
    Vec2, WidgetInfo, WidgetType,
};
use silico_core::command::Revision;
use silico_core::document::FeatureId;
use silico_core::naming::Name;
use silico_protocol::Mesh;
use silico_render::render::{Hit, MAX_HITS, Pickable, hits};

use crate::icons::{Icon, IconSize};
use crate::renderer::Element;
use crate::selection::{How, Selection, centre_of, square};
use crate::theme;
use crate::viewport::{LeftEvent, LeftKind, SecondaryClick, View, ViewFrame};

/// The context menu's one item, as the prototype labels it.
pub const SELECT_OTHER: &str = "Select Other…";

/// The list's heading, as the prototype has it (drawn in capitals).
pub const HEADING: &str = "Under the pointer, nearest first";

/// What the context menu's item says while the view's meshes aren't of the model the
/// selection is read against: nothing was cast.
pub const UNREAD: &str = "the model's meshes aren't shown yet";

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
    /// The canvas's context menu, *Select Other…*.
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

/// The canvas's Select Other: its menu, while one is open; each body's boxes, as the last cast
/// made them; the keys taken for the menu and not yet applied; whether a left press closed it
/// (its stream, to its end, is the menu's); and the egui pass it was last given a frame in.
#[derive(Debug, Default)]
pub struct SelectOther {
    menu: Option<Menu>,
    pickables: BTreeMap<u32, Pickable>,
    keys: Vec<MenuKey>,
    swallowing: bool,
    shown: Option<u64>,
}

/// An open menu: which; where its right click was, in points; the rows the cast found (none,
/// with `unread`, if it wasn't cast); the row lit and the one the pointer was last over; what
/// it was cast on (the selection's model, and each body drawn with its mesh); the wheel's
/// steps not yet whole; the list's scroll, whether it was just stepped, and its width.
#[derive(Debug)]
struct Menu {
    phase: Phase,
    at: Pos2,
    rows: Vec<Row>,
    unread: bool,
    lit: Option<usize>,
    pointed: Option<usize>,
    model: Option<(u64, Revision)>,
    meshes: Vec<(u32, Arc<Mesh>)>,
    wheel: f32,
    scroll: f32,
    stepped: bool,
    width: Option<f32>,
}

/// What the person did in the menu this frame.
enum Act {
    /// *Select Other…* chosen: the list.
    List,
    /// A row chosen, Ctrl held or not.
    Choose(usize, bool),
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

    /// The row lit, if one is.
    pub fn lit(&self) -> Option<usize> {
        self.menu.as_ref().and_then(|menu| menu.lit)
    }

    /// `key`, taken for the menu by the window's keys, applied when the tab's overlay is
    /// next given a frame (this one's, which comes after the keys).
    pub(crate) fn key(&mut self, key: MenuKey) {
        self.keys.push(key);
    }

    /// The canvas's right clicks and left button this frame (`frame`, given to the tab's
    /// overlay before the selection takes its own), for the menu: a right click opens it, cast
    /// on the bodies `frame` drew if `current` (`view`'s bodies of the model `selection` is
    /// read against); the keys taken for it applied, the wheel stepping its list, and it drawn
    /// over the canvas with `painter`'s context; it closed as the module says. `names` gives a
    /// feature's name, by its ID, for the rows. The left events the selection and the box
    /// select are to be given: every one but those of a press that closed the menu.
    pub(crate) fn overlay(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        frame: &ViewFrame,
        painter: &egui::Painter,
        current: bool,
        names: &dyn Fn(&FeatureId) -> Option<String>,
    ) -> Vec<LeftEvent> {
        let ctx = painter.ctx().clone();
        let before = self.state();
        let kept = self.left_kept(frame, selection, view, &ctx);
        let pass = ctx.cumulative_pass_nr();
        let shown_last = self
            .shown
            .is_some_and(|last| last.saturating_add(1) >= pass);
        self.shown = Some(pass);
        // A tab not shown (its drag's end given it with a painter drawing nothing), or one
        // that wasn't last pass: its menu is let go, not drawn where it isn't.
        if !painter.is_visible() || !shown_last {
            self.close(selection, view, &ctx);
            self.keys.clear();
            if !painter.is_visible() {
                return kept;
            }
        }
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| !menu.still(selection, view, frame))
        {
            tracing::debug!("the model or a mesh changed under Select Other: closed");
            self.close(selection, view, &ctx);
        }
        self.pressed_off(selection, view, &ctx);
        if let Some(click) = frame.secondary.last() {
            self.open(click, selection, view, frame, current, names);
        }
        for key in std::mem::take(&mut self.keys) {
            self.apply(key, selection, view, &ctx);
        }
        self.wheel(&ctx, frame);
        if let Some(act) = self.show(&ctx) {
            match act {
                Act::List => {
                    if let Some(menu) = &mut self.menu {
                        menu.list();
                    }
                }
                Act::Choose(at, ctrl) => self.choose(at, ctrl, selection, view, &ctx),
            }
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
        kept
    }

    /// Which menu is open, and its row lit: what a frame changing asks the next to show.
    fn state(&self) -> Option<(Phase, Option<usize>)> {
        self.menu.as_ref().map(|menu| (menu.phase, menu.lit))
    }

    /// `frame`'s left events but those of a press that closed the menu: a press while it's
    /// open closes it, and that press's stream (its moves, its release or cancel, and the
    /// double-click straight after its release) is the menu's.
    fn left_kept(
        &mut self,
        frame: &ViewFrame,
        selection: &mut Selection,
        view: &mut View,
        ctx: &egui::Context,
    ) -> Vec<LeftEvent> {
        let mut kept = Vec::with_capacity(frame.left.len());
        let mut ended = false;
        for event in &frame.left {
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
            if self.menu.is_some() && event.kind == LeftKind::Press {
                tracing::debug!("a press on the canvas closed Select Other");
                self.close(selection, view, ctx);
                self.swallowing = true;
                continue;
            }
            kept.push(*event);
        }
        kept
    }

    /// The menu closed by a press of any button off it, anywhere in the window: where egui's
    /// own hit test (the layer at the press) isn't the menu's, as it decides whether the canvas
    /// under it takes the press, so a press is the menu's or another's, never both or neither.
    fn pressed_off(&mut self, selection: &mut Selection, view: &mut View, ctx: &egui::Context) {
        let Some(layer) = self.menu.as_ref().map(Menu::layer) else {
            return;
        };
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
        if presses.iter().any(|at| ctx.layer_id_at(*at) != Some(layer)) {
            self.close(selection, view, ctx);
        }
    }

    /// The context menu opened at `click`, cast (if `current`) on what `frame` drew.
    fn open(
        &mut self,
        click: &SecondaryClick,
        selection: &mut Selection,
        view: &View,
        frame: &ViewFrame,
        current: bool,
        names: &dyn Fn(&FeatureId) -> Option<String>,
    ) {
        let _span = tracing::debug_span!("select_other_open", current).entered();
        let ppp = frame.pixels_per_point;
        let at = frame.rect.min + Vec2::new(click.at_px[0], click.at_px[1]) / ppp;
        let meshes: Vec<(u32, Arc<Mesh>)> = frame
            .drawn
            .iter()
            .filter_map(|n| {
                view.bodies()
                    .get(n)
                    .map(|body| (*n, Arc::clone(&body.mesh)))
            })
            .collect();
        let rows = if current {
            self.cast(click, view, frame, &meshes, names)
        } else {
            Vec::new()
        };
        tracing::debug!(rows = rows.len(), "select_other_cast");
        self.menu = Some(Menu {
            phase: Phase::Context,
            at: if at.x.is_finite() && at.y.is_finite() {
                at
            } else {
                frame.rect.center()
            },
            rows,
            unread: !current,
            lit: None,
            pointed: None,
            model: selection.model_of(),
            meshes,
            wheel: 0.0,
            scroll: 0.0,
            stepped: false,
            width: None,
        });
        selection.hold_hover(None);
    }

    /// What's under `click`'s pixel, of `meshes` (the bodies drawn, by number), as rows: each
    /// body's boxes kept from the last cast while its mesh is the same.
    fn cast(
        &mut self,
        click: &SecondaryClick,
        view: &View,
        frame: &ViewFrame,
        meshes: &[(u32, Arc<Mesh>)],
        names: &dyn Fn(&FeatureId) -> Option<String>,
    ) -> Vec<Row> {
        self.pickables.retain(|number, pickable| {
            meshes
                .iter()
                .any(|(n, mesh)| n == number && Arc::ptr_eq(mesh, pickable.mesh()))
        });
        for (number, mesh) in meshes {
            self.pickables
                .entry(*number)
                .or_insert_with(|| Pickable::new(Arc::clone(mesh)));
        }
        let bodies: Vec<(u32, &Pickable)> = meshes
            .iter()
            .filter_map(|(n, _)| self.pickables.get(n).map(|pickable| (*n, pickable)))
            .collect();
        let pixel = centre_of(square(click.at_px, frame.size_px, 0));
        let pointer = pixel.map(|c| f64::from(c) + 0.5);
        hits(
            &frame.camera,
            frame.size_px,
            frame.pixels_per_point,
            &bodies,
            pointer,
        )
        .iter()
        .filter_map(|hit| row_of(hit, view, names))
        .collect()
    }

    /// `key` applied to the open menu.
    fn apply(
        &mut self,
        key: MenuKey,
        selection: &mut Selection,
        view: &mut View,
        ctx: &egui::Context,
    ) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        match key {
            MenuKey::Close => self.close(selection, view, ctx),
            MenuKey::Next => menu.step(1),
            MenuKey::Back => menu.step(-1),
            MenuKey::Choose { ctrl } => match (menu.phase, menu.lit) {
                (Phase::Context, Some(_)) => menu.list(),
                (Phase::List, Some(at)) => self.choose(at, ctrl, selection, view, ctx),
                (_, None) => {}
            },
        }
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
        let over_menu = ctx
            .pointer_hover_pos()
            .is_some_and(|at| ctx.layer_id_at(at) == Some(menu.layer()));
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

    /// The menu closed: the hover and the wheel let go, and the keyboard's focus.
    fn close(&mut self, selection: &mut Selection, view: &mut View, ctx: &egui::Context) {
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
                menu_frame(ui).show(ui, |ui| {
                    ui.scope_builder(UiBuilder::new().id_salt("select_other"), |ui| {
                        // Its rows stacked, as the prototype's are: each its own padding.
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                            node.set_role(egui::accesskit::Role::Menu);
                            node.set_label("Select Other");
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

impl Menu {
    /// Whether what it was cast on is still what's drawn: the selection's model, and each
    /// body drawn with the same mesh.
    fn still(&self, selection: &Selection, view: &View, frame: &ViewFrame) -> bool {
        self.model == selection.model_of()
            && self.meshes.len() == frame.drawn.len()
            && self
                .meshes
                .iter()
                .zip(&frame.drawn)
                .all(|((n, mesh), drawn)| {
                    n == drawn
                        && view
                            .bodies()
                            .get(n)
                            .is_some_and(|body| Arc::ptr_eq(&body.mesh, mesh))
                })
    }

    /// The layer it's drawn on: each of its two menus an area of its own.
    fn layer(&self) -> egui::LayerId {
        let which = match self.phase {
            Phase::Context => "context",
            Phase::List => "list",
        };
        egui::LayerId::new(Order::Foreground, menu_id().with(which))
    }

    /// Whether its context menu's item can be chosen: something is under the pointer.
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

    /// The lit row moved `by` rows, round the list's ends; in the context menu, its item lit
    /// (if it can be chosen).
    fn step(&mut self, by: i64) {
        if self.phase == Phase::Context {
            if self.listable() {
                self.lit = Some(0);
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

/// `hit` as a row: named as its body's mesh names it, its note its kind, the feature that
/// made it (by its name, as `names` gives it, or its ID), and whether it's hidden; none if
/// its body or its name isn't the view's.
fn row_of(hit: &Hit, view: &View, names: &dyn Fn(&FeatureId) -> Option<String>) -> Option<Row> {
    let named = view.bodies().get(&hit.body)?.name_of(hit.element)?;
    let (kind, number) = match hit.element {
        Element::Face(n) => ("Face", n),
        Element::Edge(n) => ("Edge", n),
        Element::Vertex(n) => ("Vertex", n),
    };
    let maker = named.name.feature();
    let made_by = names(maker).unwrap_or_else(|| maker.as_str().to_owned());
    let mut note = format!("{kind} · {made_by}");
    if hit.hidden {
        note.push_str(" · hidden");
    }
    Some(Row {
        body: hit.body,
        element: hit.element,
        name: Arc::clone(&named.name),
        stable: named.stable,
        hidden: hit.hidden,
        label: format!("{kind}{number}"),
        note,
    })
}

/// The context menu: *Select Other…*, how many are under the pointer (or why none were
/// counted), off with none.
fn context_ui(ui: &mut Ui, menu: &mut Menu, act: &mut Option<Act>) {
    let note = if menu.unread {
        UNREAD.to_owned()
    } else {
        match menu.rows.len() {
            0 => "nothing under the pointer".to_owned(),
            n if n >= MAX_HITS => format!("the nearest {n} under the pointer"),
            n => format!("{n} under the pointer"),
        }
    };
    let enabled = menu.listable();
    let width = row_width(ui, SELECT_OTHER, &note).max(least_width());
    let response = paint_row(
        ui,
        width,
        Some(Icon::Target),
        SELECT_OTHER,
        &note,
        enabled,
        menu.lit == Some(0),
    );
    if enabled && response.hovered() {
        menu.lit = Some(0);
    }
    if enabled && response.clicked() {
        *act = Some(Act::List);
    }
}

/// The list: its heading, its rows (scrolling past [`theme::SELECT_OTHER_ROWS`], the lit row
/// kept in view as it's stepped to), and its foot.
fn list_ui(ui: &mut Ui, menu: &mut Menu, act: &mut Option<Act>) {
    // Its rows' widest, laid out once: they don't change while it's open.
    let width = *menu.width.get_or_insert_with(|| {
        menu.rows
            .iter()
            .map(|row| row_width(ui, &row.label, &row.note))
            .fold(least_width(), f32::max)
            .max(foot_width(ui))
    });
    paint_head(ui, width, HEADING);
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
            let response = paint_row(
                ui,
                width,
                Some(icon_of(each.element)),
                &each.label,
                &each.note,
                true,
                menu.lit == Some(at),
            );
            if response.hovered() {
                pointed = Some(at);
            }
            if response.clicked() {
                *act = Some(Act::Choose(at, ctrl));
            }
        }
    });
    menu.scroll = output.state.offset.y;
    // A row the pointer has moved onto lights (the prototype's mouseover); one it rests on
    // doesn't take the light back from the keys.
    if pointed.is_some() && pointed != menu.pointed {
        menu.lit = pointed;
    }
    menu.pointed = pointed;
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

/// A menu's frame (`.menu`): egui's, as the theme sets it for every menu (the feature
/// menus' and the toolbar's drop-downs): Gray.200 inside a Gray.250 border, `--r`'s
/// corners, the popups' shadow, padded [`theme::MENU_PADDING`].
fn menu_frame(ui: &Ui) -> egui::Frame {
    egui::Frame::menu(ui.style())
}

/// The least a menu's rows are wide: [`theme::MENU_MIN_WIDTH`] with its border and padding
/// inside it (`box-sizing: border-box`), as the feature menus take it.
fn least_width() -> f32 {
    theme::MENU_MIN_WIDTH - 2.0 * (f32::from(theme::MENU_PADDING) + theme::BORDER_WIDTH)
}

/// The width a row of `label` and `note` takes (`.menu .mi`): its padding, its icon, and its
/// words, its note dim after them (a gap and its margin, the prototype's `subdued` span).
fn row_width(ui: &Ui, label: &str, note: &str) -> f32 {
    let words = text_width(ui, label, theme::BODY_SIZE);
    let note = text_width(ui, note, theme::MENU_ITEM_SUB_SIZE);
    2.0 * theme::MENU_ITEM_PADDING
        + IconSize::Small.points()
        + theme::MENU_ITEM_GAP
        + words
        + theme::MENU_ITEM_GAP
        + theme::MENU_ITEM_SUB_MARGIN
        + note
}

fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            FontId::proportional(size),
            theme::MENU_ITEM_TEXT,
        )
        .size()
        .x
}

/// A menu's row (`.menu .mi`), `width` wide: its icon (or the space of one), its words, and
/// its note, lit or not, on or off; its response, labelled with its words and note.
fn paint_row(
    ui: &mut Ui,
    width: f32,
    icon: Option<Icon>,
    label: &str,
    note: &str,
    enabled: bool,
    lit: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, theme::MENU_ITEM_HEIGHT), sense);
    let said = format!("{label} {note}");
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &said));
    let lit = enabled && (lit || response.hovered());
    let ink = match (enabled, lit) {
        (false, _) => theme::TEXT_DISABLED,
        (true, true) => theme::MENU_ITEM_HOVER_TEXT,
        (true, false) => theme::MENU_ITEM_TEXT,
    };
    let painter = ui.painter();
    if lit {
        painter.rect_filled(rect, theme::SMALL_CORNER_RADIUS, theme::MENU_ITEM_HOVER_BG);
    }
    let side = IconSize::Small.points();
    let icon_rect = Rect::from_min_size(
        Pos2::new(
            rect.left() + theme::MENU_ITEM_PADDING,
            rect.center().y - side / 2.0,
        ),
        Vec2::splat(side),
    );
    if let Some(icon) = icon {
        icon.image(IconSize::Small)
            .tint(ink)
            .paint_at(ui, icon_rect);
    }
    let painter = ui.painter();
    let words = painter.layout_no_wrap(
        label.to_owned(),
        FontId::proportional(theme::BODY_SIZE),
        ink,
    );
    let words_at = Pos2::new(
        icon_rect.right() + theme::MENU_ITEM_GAP,
        rect.center().y - words.size().y / 2.0,
    );
    let words_end = words_at.x + words.size().x;
    painter.galley(words_at, words, ink);
    let note = painter.layout_no_wrap(
        note.to_owned(),
        FontId::proportional(theme::MENU_ITEM_SUB_SIZE),
        theme::MENU_ITEM_SUB_TEXT,
    );
    let note_at = Pos2::new(
        words_end + theme::MENU_ITEM_GAP + theme::MENU_ITEM_SUB_MARGIN,
        rect.center().y - note.size().y / 2.0,
    );
    painter.galley(note_at, note, theme::MENU_ITEM_SUB_TEXT);
    response
}

/// A menu's heading (`.menu .mh`), `width` wide: 10 high capitals spaced as the panels'
/// are, Gray.550, 6 above its line and 2 under it, 8 in from the left, as the feature menus
/// head theirs.
fn paint_head(ui: &mut Ui, width: f32, text: &str) {
    let job = egui::text::LayoutJob::single_section(
        text.to_uppercase(),
        egui::TextFormat {
            font_id: FontId::proportional(theme::TOOLBAR_LABEL_SIZE),
            color: theme::TOOLBAR_LABEL_TEXT,
            extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    let height = theme::MENU_HEAD_TOP + theme::MENU_LINE_HEIGHT + theme::MENU_HEAD_BOTTOM;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
    let line = rect.top() + theme::MENU_HEAD_TOP;
    let at = Pos2::new(
        rect.left() + theme::MENU_ITEM_PADDING,
        line + (theme::MENU_LINE_HEIGHT - galley.size().y) / 2.0,
    );
    ui.painter().galley(at, galley, theme::TOOLBAR_LABEL_TEXT);
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
                text_width(ui, text, theme::MENU_FOOT_SIZE)
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
        Menu {
            phase,
            at: Pos2::ZERO,
            rows: (1..=u32::try_from(rows).unwrap()).map(row).collect(),
            unread: false,
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
        // An empty list lights nothing; a context menu lights its item if it can be chosen.
        let mut empty = menu(0, Phase::List);
        empty.step(1);
        assert_eq!(empty.lit, None);
        let mut context = menu(2, Phase::Context);
        context.step(-1);
        assert_eq!(context.lit, Some(0));
        let mut off = menu(0, Phase::Context);
        off.step(1);
        assert_eq!(off.lit, None);
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
}
