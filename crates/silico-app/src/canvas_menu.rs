//! The canvas's context menu (M5.7; docs/design/m5.0-app.md, section 9's *Selection*): what a
//! right click on the canvas opens in selection mode, as the approved prototype's `onContext`
//! (js/interact.js) builds it, drawn as every menu is (`crate::menu`). Over an element, what a
//! click there would select (the selection's hover): headed by it and its kind ("Face6 ·
//! Face"), then *Select Other…* with how many are under the pointer, Edit and the feature
//! that made it, Sketch on this face and Look At for a planar face, a separator, and Hide and
//! its body.
//! Over nothing: Fit (F6), Home view (Home), *Select Other…*, a separator, Clear selection
//! (Esc), and Create Sketch. Where each item's action goes:
//! - *Select Other…* opens the list of what's under the pointer (`crate::select_other`,
//!   which holds this menu while it's open);
//! - Fit, Home view, and Clear selection are the registry's commands (`crate::shortcuts`,
//!   their caps its), done by the tab ([`CanvasAct`]) as their keys do them;
//! - Edit is the feature menus' Edit feature (`crate::feature_menu`, off with its reason
//!   where that is);
//! - Sketch on this face is the selection view's button (`crate::panes`): Create Sketch on
//!   the face, by the name a store writes for it (`Tab::create_sketch_on_face`). It's there
//!   once `elements` says the face is a plane, as the button is: the face's entry read when
//!   the menu opens over it (the tab's, through `crate::measures`), and none over a face of
//!   another type, an edge, or a vertex, as the prototype filters it. It's off, as the
//!   button is, while sketch mode can't open (`crate::feature_menu::sketch_off`, its
//!   reason) or while none of the face's names is stable (the store's refusal, its reason);
//! - Look At, for the same planar face, is the registry's (Ctrl+8): the view square to that
//!   face, from the outward normal its entry gives, as Ctrl+8's `Tab::look_at` turns to the
//!   face selected (`Tab::look_at_face`, the one rule), without selecting it (the prototype
//!   selects it first, to look at the selection; the menu already has the face). It's never
//!   off;
//! - Hide is as a body's eye in the browser hides it (with no key's cap: H, the registry's
//!   Hide, hides the feature selected, not the body under the pointer);
//! - Create Sketch is the toolbar's: chosen, it closes the menu and begins the pick of a
//!   plane or a flat face (`crate::plane_pick`, `Tab::begin_plane_pick`), as the
//!   prototype's item runs its command; on and off as the toolbar's is (`Tab::can_sketch`),
//!   off with sketch mode's reason.
//!
//! Left out of the prototype's items, none a command the app has yet:
//! - SEAM: Fillet, an edge's, whose dialog opens from MODIFY (`crate::toolbar`, through the
//!   app's remembered inputs), not from a command of the registry;
//! - SEAM: Copy name ("as agents write it"), no command of the registry copies a name.

use std::sync::Arc;

use eframe::egui::{self, Response, Ui, Vec2};
use silico_core::document::FeatureId;
use silico_core::naming::Name;

use crate::icons::Icon;
use crate::menu;
use crate::shortcuts::Command;

/// *Select Other…*, as the prototype labels it.
pub const SELECT_OTHER: &str = "Select Other…";

/// What *Select Other…* says while the view's meshes aren't of the model the selection is
/// read against: nothing was cast.
pub const UNREAD: &str = "the model's meshes aren't shown yet";

/// What *Select Other…* says when a body's boxes to cast on can't be made (`crate::boxes`:
/// the thread that makes them couldn't be started, or ended): nothing was cast.
pub const UNCAST: &str = "what's under the pointer can't be found";

/// One of the menu's items: *Select Other…*, which the menu does itself; Sketch on this
/// face, with the name a store writes for its face, if one can be written (none: the item
/// is off, with the store's refusal); Create Sketch, whose pick it begins; or one the tab
/// does once it's chosen.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasItem {
    SelectOther,
    SketchOnFace(Option<Arc<Name>>),
    CreateSketch,
    Act(CanvasAct),
}

/// What a chosen item asks the tab to do, the menu closed.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasAct {
    /// Edit feature, of the feature that made the element under the pointer.
    Edit(FeatureId),
    /// Sketch on this face: a sketch created on the face under the pointer, by the name a
    /// store writes for it.
    SketchOnFace(Arc<Name>),
    /// Look At: the view square to the planar face under the pointer, from its outward unit
    /// normal (its `elements` entry's).
    LookAt([f64; 3]),
    /// Hide, of the body under the pointer (by the feature that ends its chain).
    Hide(FeatureId),
    Fit,
    Home,
    ClearSelection,
    /// Create Sketch: its pick of a plane or a flat face begun.
    CreateSketch,
}

/// An item as the menu shows it: what it is, its icon, its words, its dim words after them,
/// its keys' caps, and why it's off, if it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub item: CanvasItem,
    pub icon: Option<Icon>,
    pub label: String,
    pub sub: Option<String>,
    pub caps: Vec<String>,
    pub off: Option<String>,
}

impl Shown {
    fn of(item: CanvasItem, icon: Option<Icon>, label: impl Into<String>) -> Shown {
        Shown {
            item,
            icon,
            label: label.into(),
            sub: None,
            caps: Vec::new(),
            off: None,
        }
    }

    /// It with `command`'s caps, as the platform names them.
    fn keys(self, command: Command, ctx: &egui::Context) -> Shown {
        Shown {
            caps: command.caps(ctx),
            ..self
        }
    }

    /// As `crate::menu` draws it, lit or not.
    fn line(&self, lit: bool) -> menu::Item<'_> {
        menu::Item {
            icon: self.icon,
            label: &self.label,
            sub: self.sub.as_deref(),
            caps: &self.caps,
            off: self.off.as_deref(),
            danger: false,
            lit,
            checked: None,
        }
    }

    /// What a screen reader (and a test) finds it by: its words, then its dim words (its
    /// keys are its keyboard shortcut, `crate::menu::item`).
    pub fn said(&self) -> String {
        self.line(false).said()
    }
}

/// What's under the pointer at the right click, as the menu says it: the element a click
/// there would select, by its words ("Face6") and its kind ("Face"); the feature that made
/// it, by its ID and its name; its body, by the feature that ends its chain and its name;
/// and, for a face `elements` has said is a plane, what Sketch on this face and Look At need
/// of it (none for another face, an edge, a vertex, or a face not read yet). None over
/// nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Under {
    pub label: String,
    pub kind: &'static str,
    pub maker: FeatureId,
    pub maker_name: String,
    pub body: FeatureId,
    pub body_name: String,
    pub plane: Option<Planar>,
}

/// A planar face under the pointer: the name a store writes for it (`Selected::stored`), or,
/// if none of its names is stable, why none can be written (the store's refusal, as its
/// Stability card says it); and its outward unit normal, as its `elements` entry gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Planar {
    pub stored: Result<Arc<Name>, String>,
    pub normal: [f64; 3],
}

/// What the menu says of the rest: how many are under the pointer (*Select Other…*'s dim
/// words) and whether there's any to list; why Edit can't open the maker's dialog, if it
/// can't; whether anything is selected; and why sketch mode can't open, if it can't (Sketch
/// on this face's and Create Sketch's reason).
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub count: String,
    pub listable: bool,
    pub edit_off: Option<String>,
    pub selected: bool,
    pub sketch_off: Option<String>,
}

/// The canvas's context menu: its head, if it's over an element, and its items, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasMenu {
    pub head: Option<String>,
    pub items: Vec<Shown>,
}

/// What a frame of the menu drawn did: the item the pointer is over and the one clicked.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Drawn {
    pub pointed: Option<usize>,
    /// The item the pointer is on, whether it's on or off (`pointed` is the one on): taken
    /// from the item's rect holding the pointer, since egui never calls a disabled widget
    /// hovered.
    pub hovered: Option<usize>,
    pub clicked: Option<usize>,
}

impl CanvasMenu {
    /// The menu over `under` (or over nothing), as the prototype's `onContext` builds it,
    /// its caps as `ctx`'s platform names them.
    pub fn new(under: Option<&Under>, context: &Context, ctx: &egui::Context) -> CanvasMenu {
        let mut select_other = Shown::of(CanvasItem::SelectOther, Some(Icon::Target), SELECT_OTHER);
        select_other.sub = Some(context.count.clone());
        if !context.listable {
            select_other.off = Some("Nothing is under the pointer".to_owned());
        }
        match under {
            Some(under) => {
                let mut edit = Shown::of(
                    CanvasItem::Act(CanvasAct::Edit(under.maker.clone())),
                    Some(Icon::Edit),
                    format!("Edit {}", under.maker_name),
                );
                edit.off = context.edit_off.clone();
                // Off as the selection view's button is: while sketch mode can't open, or
                // while no store can name the face.
                let sketch = under.plane.as_ref().map(|plane| {
                    let mut sketch = Shown::of(
                        CanvasItem::SketchOnFace(plane.stored.clone().ok()),
                        Some(Icon::Sketch),
                        crate::panes::SKETCH_ON_FACE,
                    );
                    sketch.off = context
                        .sketch_off
                        .clone()
                        .or_else(|| plane.stored.clone().err());
                    sketch
                });
                // Never off: it needs nothing but the face's normal.
                let look = under.plane.as_ref().map(|plane| {
                    Shown::of(
                        CanvasItem::Act(CanvasAct::LookAt(plane.normal)),
                        Some(Icon::LookAt),
                        Command::LookAt.label(),
                    )
                    .keys(Command::LookAt, ctx)
                });
                // No key's cap: H hides the feature selected, not the body under the
                // pointer this hides.
                let hide = Shown::of(
                    CanvasItem::Act(CanvasAct::Hide(under.body.clone())),
                    Some(Icon::EyeOff),
                    format!("Hide {}", under.body_name),
                );
                CanvasMenu {
                    head: Some(format!("{} · {}", under.label, under.kind)),
                    items: [Some(select_other), Some(edit), sketch, look, Some(hide)]
                        .into_iter()
                        .flatten()
                        .collect(),
                }
            }
            None => {
                let fit = Shown::of(
                    CanvasItem::Act(CanvasAct::Fit),
                    Some(Icon::Fit),
                    Command::Fit.label(),
                )
                .keys(Command::Fit, ctx);
                let home = Shown::of(
                    CanvasItem::Act(CanvasAct::Home),
                    Some(Icon::Home),
                    Command::Home.label(),
                )
                .keys(Command::Home, ctx);
                let mut clear = Shown::of(
                    CanvasItem::Act(CanvasAct::ClearSelection),
                    None,
                    Command::ClearSelection.label(),
                )
                .keys(Command::ClearSelection, ctx);
                if !context.selected {
                    clear.off = Some("Nothing is selected".to_owned());
                }
                let mut sketch = Shown::of(
                    CanvasItem::CreateSketch,
                    Some(Icon::Sketch),
                    crate::toolbar::CREATE_SKETCH_WORDS,
                );
                sketch.off = context.sketch_off.clone();
                CanvasMenu {
                    head: None,
                    items: vec![fit, home, select_other, clear, sketch],
                }
            }
        }
    }

    /// Whether a separator comes before its item `at` (the prototype's `'sep'`: before Hide,
    /// and before Clear selection).
    fn separated(&self, at: usize) -> bool {
        self.items.get(at).is_some_and(|shown| {
            matches!(
                shown.item,
                CanvasItem::Act(CanvasAct::Hide(_) | CanvasAct::ClearSelection)
            )
        })
    }

    /// Whether its item `at` can be chosen.
    pub fn on(&self, at: usize) -> bool {
        self.items.get(at).is_some_and(|shown| shown.off.is_none())
    }

    /// The item lit `by` steps on from `lit` (none lit: the first step forward lights the
    /// first, back the last), round its ends, passing over those that are off; none if none
    /// is on.
    pub fn step(&self, lit: Option<usize>, by: i64) -> Option<usize> {
        let on: Vec<usize> = (0..self.items.len()).filter(|&at| self.on(at)).collect();
        let count = i64::try_from(on.len()).ok().filter(|&count| count > 0)?;
        let from = match lit.and_then(|lit| on.iter().position(|&at| at == lit)) {
            Some(place) => i64::try_from(place).ok()?,
            None if by >= 0 => -1,
            None => count,
        };
        // Taken round first, so no step overflows.
        let to = (from + by.rem_euclid(count)).rem_euclid(count);
        on.get(usize::try_from(to).ok()?).copied()
    }

    /// Its lines' width: its widest line's, its head's too, and a menu's least at least
    /// (`crate::menu::width`).
    fn width(&self, ui: &Ui) -> f32 {
        let lines: Vec<menu::Item<'_>> = self.items.iter().map(|shown| shown.line(false)).collect();
        menu::width(ui, self.head.as_deref(), &lines, 0.0)
    }

    /// It drawn, its item `lit` lit. What was pointed at or clicked.
    pub fn draw(&self, ui: &mut Ui, lit: Option<usize>) -> Drawn {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let width = self.width(ui);
        if let Some(head) = &self.head {
            menu::head(ui, head, width);
        }
        let mut drawn = Drawn::default();
        for (at, shown) in self.items.iter().enumerate() {
            if self.separated(at) {
                menu::separator(ui, width);
            }
            let response: Response = menu::item(ui, &shown.line(lit == Some(at)), width);
            if response.contains_pointer() {
                drawn.hovered = Some(at);
            }
            if shown.off.is_none() && response.hovered() {
                drawn.pointed = Some(at);
            }
            if response.clicked() {
                drawn.clicked = Some(at);
            }
        }
        drawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fid(id: &str) -> FeatureId {
        FeatureId::new(id).unwrap()
    }

    fn under() -> Under {
        Under {
            label: "Face6".to_owned(),
            kind: "Face",
            maker: fid("k7f2qx3m"),
            maker_name: "Block".to_owned(),
            body: fid("p0000001"),
            body_name: "Plate".to_owned(),
            plane: None,
        }
    }

    /// The menu's facts: something under the pointer to list or not, something selected or
    /// not, and sketch mode able to open.
    fn context(listable: bool, selected: bool) -> Context {
        Context {
            count: if listable {
                "4 under the pointer".to_owned()
            } else {
                "nothing under the pointer".to_owned()
            },
            listable,
            edit_off: None,
            selected,
            sketch_off: None,
        }
    }

    /// `context`, sketch mode unable to open, saying why.
    fn unsketchable(context: Context) -> Context {
        Context {
            sketch_off: Some("Close the open dialog first".to_owned()),
            ..context
        }
    }

    /// Each item as a screen reader has it: its name, its keyboard shortcut, and whether
    /// it's on.
    fn said(menu: &CanvasMenu, ctx: &egui::Context) -> Vec<(String, Option<String>, bool)> {
        menu.items
            .iter()
            .map(|shown| {
                let line = shown.line(false);
                (line.said(), line.shortcut(ctx), shown.off.is_none())
            })
            .collect()
    }

    /// An item as [`said`] gives it.
    fn item(said: &str, keys: Option<&str>, on: bool) -> (String, Option<String>, bool) {
        (said.to_owned(), keys.map(str::to_owned), on)
    }

    #[test]
    fn over_an_element_and_over_nothing_its_items_are_the_prototypes_in_its_order() {
        // js/interact.js's `onContext`, of the items the app has commands for: over an
        // element its head (the element and its kind), Select Other… with its count, Edit
        // and the maker, and Hide and the body, with no key (H hides the feature selected,
        // not the body under the pointer); over nothing Fit with F6, Home view
        // with Home, Select Other…, Clear selection with Esc (off with nothing selected), and
        // Create Sketch, last (G7).
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Nix);
        let over = CanvasMenu::new(Some(&under()), &context(true, false), &ctx);
        assert_eq!(over.head.as_deref(), Some("Face6 · Face"));
        assert_eq!(
            said(&over, &ctx),
            [
                item("Select Other… 4 under the pointer", None, true),
                item("Edit Block", None, true),
                item("Hide Plate", None, true),
            ]
        );
        assert_eq!(
            over.items
                .iter()
                .map(|s| s.item.clone())
                .collect::<Vec<_>>(),
            [
                CanvasItem::SelectOther,
                CanvasItem::Act(CanvasAct::Edit(fid("k7f2qx3m"))),
                CanvasItem::Act(CanvasAct::Hide(fid("p0000001"))),
            ]
        );
        assert!(over.items[2].caps.is_empty(), "{:?}", over.items[2]);
        assert!(!over.separated(1) && over.separated(2));
        let off = CanvasMenu::new(None, &context(false, false), &ctx);
        assert_eq!(off.head, None);
        assert_eq!(
            said(&off, &ctx),
            [
                item("Fit", Some("F6"), true),
                item("Home view", Some("Home"), true),
                item("Select Other… nothing under the pointer", None, false),
                item("Clear selection", Some("Esc"), false),
                item("Create Sketch", None, true),
            ]
        );
        assert!(off.separated(3) && !off.separated(2) && !off.separated(4));
        let selected = CanvasMenu::new(None, &context(true, true), &ctx);
        assert!(selected.on(2) && selected.on(3));
        // Edit off says why, as the feature menus' Edit feature does.
        let mut busy = context(true, false);
        busy.edit_off = Some("Close the open dialog first".to_owned());
        let over = CanvasMenu::new(Some(&under()), &busy, &ctx);
        assert_eq!(
            over.items[1].off.as_deref(),
            Some("Close the open dialog first")
        );
    }

    #[test]
    fn a_step_goes_round_the_items_that_are_on_passing_over_those_off() {
        let ctx = egui::Context::default();
        // Over nothing, nothing selected, nothing under the pointer, sketch mode unable to
        // open: Fit and Home on, Select Other…, Clear selection, and Create Sketch off.
        let menu = CanvasMenu::new(None, &unsketchable(context(false, false)), &ctx);
        assert_eq!(menu.step(None, 1), Some(0));
        assert_eq!(menu.step(None, -1), Some(1));
        assert_eq!(menu.step(Some(1), 1), Some(0));
        assert_eq!(menu.step(Some(0), -1), Some(1));
        assert_eq!(menu.step(Some(0), i64::MAX), Some(1));
        assert_eq!(menu.step(Some(0), i64::MIN), Some(0));
        // Everything on but Create Sketch: round those four.
        let all = CanvasMenu::new(None, &unsketchable(context(true, true)), &ctx);
        assert_eq!(all.step(Some(3), 1), Some(0));
        assert_eq!(all.step(Some(1), 2), Some(3));
        // From none lit, the first step forward lights the first, two the second; back, the
        // last.
        assert_eq!(all.step(None, 1), Some(0));
        assert_eq!(all.step(None, 2), Some(1));
        assert_eq!(all.step(None, -1), Some(3));
        // An item lit that's off (it went off while lit) steps from none.
        assert_eq!(menu.step(Some(2), 1), Some(0));
        // None on: none lit.
        let mut none = CanvasMenu::new(None, &unsketchable(context(false, false)), &ctx);
        for shown in &mut none.items {
            shown.off = Some("off".to_owned());
        }
        assert_eq!(none.step(None, 1), None);
        assert_eq!(none.step(Some(0), -1), None);
    }

    #[test]
    fn drawn_its_lines_are_as_wide_as_its_widest_and_the_item_lit_alone_is_filled_blue() {
        // styles.css's `.menu .mi` (`padding: 0 8px; gap: 8px`, its icon 14 wide, its dim
        // words `margin-left: 8px; font-size: 11px`, `.sc` after a gap, `padding-left: 24px`,
        // its caps `.kbd`s, each as wide as its words and its padding and borders (10), or
        // 16, 2 apart) in a `.menu` at least `min-width: 200px` less its border and its 4 of
        // padding each side (190). Every item on, Home view (the second) lit: it alone filled
        // `--blue` (#186add), the menu's lines' width, 24 high, 24 below the first's top.
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let menu = CanvasMenu::new(None, &context(true, true), &ctx);
        let mut drawn = Vec::new();
        let mut widest = 0.0f32;
        for time in 1..=2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::splat(1000.0),
                )),
                time: Some(f64::from(time)),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let text = |text: &str, size: f32| {
                    ui.painter()
                        .layout_no_wrap(
                            text.to_owned(),
                            egui::FontId::proportional(size),
                            egui::Color32::WHITE,
                        )
                        .size()
                        .x
                };
                widest = menu
                    .items
                    .iter()
                    .map(|shown| {
                        let sub = shown
                            .sub
                            .as_deref()
                            .map_or(0.0, |sub| 8.0 + 8.0 + text(sub, 11.0));
                        let caps: f32 = shown
                            .caps
                            .iter()
                            .map(|cap| (text(cap, 10.5) + 10.0).max(16.0))
                            .sum::<f32>()
                            + 2.0 * (shown.caps.len().saturating_sub(1) as f32);
                        16.0 + 14.0 + 8.0 + text(&shown.label, 12.0) + sub + 8.0 + 24.0 + caps
                    })
                    .fold(190.0, f32::max);
                let place =
                    egui::Rect::from_min_size(egui::Pos2::new(10.0, 20.0), Vec2::new(900.0, 900.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(place), |ui| {
                    assert_eq!(menu.draw(ui, Some(1)), Drawn::default());
                });
            });
            output.textures_delta.clear();
            drawn = output.shapes;
        }
        assert!(widest > 190.0, "{widest}");
        let blue: Vec<egui::Rect> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.fill == egui::Color32::from_rgb(0x18, 0x6a, 0xdd) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        assert_eq!(blue.len(), 1, "{blue:?}");
        let want = egui::Rect::from_min_size(egui::Pos2::new(10.0, 44.0), Vec2::new(widest, 24.0));
        assert!(
            (blue[0].min - want.min).length() < 1e-3 && (blue[0].max - want.max).length() < 1e-3,
            "{blue:?}, {want:?}"
        );
    }

    #[test]
    fn over_a_planar_face_sketch_on_this_face_follows_edit_and_is_off_as_the_selection_views_button_is()
     {
        // js/interact.js's `onContext` over a face `ref.kind === 'plane'`: Sketch on this
        // face, with the sketch icon, after Edit, then Look At, with its icon and Ctrl+8, then
        // the separator and Hide. Sketch on this face is on, by the name a store writes for
        // the face, as the selection view's button (panes.rs) is; off while sketch mode can't
        // open, saying why, or while no store can name the face, saying the store's refusal;
        // sketch mode's reason first. Look At is on all the same, turning to the face's
        // normal.
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Nix);
        // A unit normal with three distinct parts (0.48² + 0.6² + 0.64² = 1), so the act
        // carries the face's own, in order.
        const TILTED: [f64; 3] = [0.48, 0.6, 0.64];
        let top = || {
            Arc::new(Name::Element {
                feature: fid("k7f2qx3m"),
                made: silico_core::naming::TypeWord::Primitive(
                    silico_core::naming::PrimitiveKind::Box,
                ),
                element: silico_protocol::ElementRef {
                    kind: silico_protocol::ElementKind::Face,
                    index: 6,
                },
            })
        };
        // The top, its stored name the store's, or none with the store's refusal.
        let face = |refused: Option<&str>| Under {
            plane: Some(Planar {
                stored: refused.map_or_else(|| Ok(top()), |why| Err(why.to_owned())),
                normal: TILTED,
            }),
            ..under()
        };
        let over = CanvasMenu::new(Some(&face(None)), &context(true, false), &ctx);
        assert_eq!(
            said(&over, &ctx),
            [
                item("Select Other… 4 under the pointer", None, true),
                item("Edit Block", None, true),
                item("Sketch on this face", None, true),
                item("Look At", Some("Ctrl+8"), true),
                item("Hide Plate", None, true),
            ]
        );
        assert_eq!(over.items[2].item, CanvasItem::SketchOnFace(Some(top())));
        assert_eq!(over.items[2].icon, Some(Icon::Sketch));
        assert!(over.items[2].caps.is_empty());
        assert_eq!(
            over.items[3].item,
            CanvasItem::Act(CanvasAct::LookAt(TILTED))
        );
        assert_eq!(over.items[3].icon, Some(Icon::LookAt));
        assert!(!over.separated(2) && !over.separated(3) && over.separated(4));
        // Look At on while Sketch on this face is off, for either reason, the rest as they
        // were.
        for (under, context) in [
            (face(None), unsketchable(context(true, false))),
            (face(Some("unstable")), context(true, false)),
        ] {
            let menu = CanvasMenu::new(Some(&under), &context, &ctx);
            assert_eq!(
                said(&menu, &ctx),
                [
                    item("Select Other… 4 under the pointer", None, true),
                    item("Edit Block", None, true),
                    item("Sketch on this face", None, false),
                    item("Look At", Some("Ctrl+8"), true),
                    item("Hide Plate", None, true),
                ]
            );
            assert!(menu.on(3));
            assert_eq!(
                menu.items[3].item,
                CanvasItem::Act(CanvasAct::LookAt(TILTED))
            );
        }
        let off = |under: &Under, context: &Context| {
            CanvasMenu::new(Some(under), context, &ctx).items[2]
                .off
                .clone()
        };
        let why = "Close the open dialog first";
        assert_eq!(
            off(&face(None), &unsketchable(context(true, false))).as_deref(),
            Some(why)
        );
        let unstable = "k7f2qx3m.box.Face6 is unstable";
        assert_eq!(
            off(&face(Some(unstable)), &context(true, false)).as_deref(),
            Some(unstable)
        );
        assert_eq!(
            off(&face(Some(unstable)), &unsketchable(context(true, false))).as_deref(),
            Some(why)
        );
        // A face not known to be a plane (another type, or not read yet), an edge, or a
        // vertex: no item.
        let plain = CanvasMenu::new(Some(&under()), &context(true, false), &ctx);
        assert!(plain.items.iter().all(|shown| !matches!(
            shown.item,
            CanvasItem::SketchOnFace(_) | CanvasItem::Act(CanvasAct::LookAt(_))
        )));
    }

    #[test]
    fn over_nothing_create_sketch_is_last_with_the_sketch_icon_and_off_with_sketch_modes_reason() {
        // js/interact.js's `onContext` over nothing: Create Sketch, the sketch icon, no keys,
        // after Clear selection with no separator between them; on as the toolbar's is (sketch
        // mode able to open), off with why it can't; and a step reaches it when it's on.
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Nix);
        let menu = CanvasMenu::new(None, &context(false, false), &ctx);
        let last = menu.items.last().unwrap();
        assert_eq!(last.item, CanvasItem::CreateSketch);
        assert_eq!(
            (
                last.icon,
                last.label.as_str(),
                last.caps.is_empty(),
                last.off.as_deref()
            ),
            (Some(Icon::Sketch), "Create Sketch", true, None)
        );
        assert!(!menu.separated(4));
        // Fit, Home view, and Create Sketch on: back from none lights Create Sketch, and on
        // from Home view too, passing Select Other… and Clear selection, off.
        assert_eq!(menu.step(None, -1), Some(4));
        assert_eq!(menu.step(Some(1), 1), Some(4));
        assert_eq!(menu.step(Some(4), 1), Some(0));
        let off = CanvasMenu::new(None, &unsketchable(context(false, false)), &ctx);
        assert_eq!(
            off.items[4].off.as_deref(),
            Some("Close the open dialog first")
        );
    }
}
