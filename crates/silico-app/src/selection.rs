//! The tab's selection (M5.7; docs/design/m5.0-app.md, section 9's *Selection*): what the
//! person has selected, as SolidWorks selects (docs/ux/solidworks.md, §1: a click selects
//! what's under the pointer, Ctrl adds or removes it, a click on empty space or Esc
//! clears), and what the pointer is over (*Hover*). The app holds the state; the rules are
//! the core's ([`silico_core::selection`]): each pick read at once against the published
//! model, kept, flagged if its name isn't stable, with the refusal a store would give; and,
//! once a rebuild publishes a new model, carried to it, what's gone dropped and said, the
//! rest kept (as SolidWorks' selection box keeps its valid items). Each name is read as it
//! came to the model: a pick's as picked, one carried from an older model as carried, its
//! unstable text gone, until the next model.
//!
//! A selection is a list of items in the order picked: features (a row of the browser or
//! the timeline, by ID) and elements (a face or an edge of the canvas, by name, as an agent
//! writes them: the stable name the core holds for each, [`Selected::name`]). Each
//! element is found (the core's answer, which the canvas draws) or waiting: its body's
//! elements aren't named yet (the core's pending, asked again each frame), or its feature
//! didn't build (asked again of the next model published).
//!
//! The canvas's clicks and hover go through [`Selection::overlay`]: a left press and its
//! release within egui's click distance is a click, and a click asks the renderer to pick
//! under it, asking again each frame until an answer to a frame since the click comes (an
//! egui pass that's discarded takes the request and paints nothing, and a renderer backing
//! off makes none), a newer click supersedes it, the camera moves, or [`CLICK_FRAMES`] go
//! by. A hover asks again each frame the pointer moves to another pixel, but while a drag
//! navigates; what it found is let go when the camera or the bodies change under it, and
//! answers asked before that change are passed over. A pick takes an edge within
//! [`EDGE_BAND_POINTS`] of the pointer before the face under it, as SolidWorks takes an
//! edge near the pointer.
//!
//! A pick is named, and the selection's elements drawn, only on bodies read from the model
//! the selection is read against ([`View::bodies_of`]): a model published, its meshes not
//! read yet, a click waits for them, asking nothing, the hover is let go, and no element's
//! number of the new model is drawn on the old model's meshes. A click waits for the
//! meshes of one model, however long they take, counting no frames (a slow read on a busy
//! machine is no reason to drop it); it's let go once what it was aimed at, or what it
//! would be picked on, changes: the camera moves (its pixel was of the view before),
//! another model is published before the meshes it waits for come, the filter changes, a
//! mode claims the canvas ([`Selection::canvas_claimed`]), or the selection is cleared or
//! set from elsewhere; and once no meshes are coming: their read refused
//! ([`View::refused_of`]), or no model built. A read in flight that never answers is let go
//! after [`CLICK_WAIT`], a backstop. Each let go the person didn't do is said, in its
//! cause's words, never over what a rebuild left gone from the same model.
//!
//! The visuals are behind [`outlines`], which the UI port replaces.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use silico_core::command::{Problems, Revision};
use silico_core::document::FeatureId;
use silico_core::naming::Name;
use silico_core::recompute::Model;
use silico_core::selection::{self as rules, Carry, Gone, Resolved, Selected};
use silico_core::{Error, ErrorCode};
use silico_protocol::{ElementKind, ElementRef};

use crate::renderer::{Element, Outlines, Picking, PixelRect};
use crate::theme;
use crate::viewport::{LeftKind, View, ViewBody, ViewFrame};

/// The parameter a selection's names are read at: an error of its third name is at
/// `selection.2`.
pub const PARAMETER: &str = "selection";

/// How far from the pointer an edge is taken before the face under it, in points: the
/// picking rectangle is a square this far out on each side of the pointer's pixel.
pub const EDGE_BAND_POINTS: f32 = 3.0;

/// How many frames a click waits for its pick's answer, asking again each frame: past
/// this (a second at 60 Hz: a renderer that answers none, or none installed), it's let go,
/// so the frames asked for while it waits stop. A frame is the unit: each frame drawn is
/// what answers a pick. Frames waited for the meshes of the selection's model aren't
/// counted (that wait goes by what changes, and by [`CLICK_WAIT`]).
pub const CLICK_FRAMES: u32 = 60;

/// The backstop of a click's wait for the meshes of the selection's model, from the click,
/// by the viewport's clock: a read in flight that never answers. Never reached in normal
/// use, where the read answers, is refused, or is overtaken by another model first (each
/// of which ends the wait as it happens).
pub const CLICK_WAIT: Duration = Duration::from_secs(10);

/// What the status bar says of a click let go at [`CLICK_WAIT`], its meshes still unread.
pub const CLICK_STALE: &str = "The click was let go: the model's meshes took too long to read; \
                               click again once they're shown";

/// What the status bar says of a click let go as another model was published before the
/// meshes it waited for came: what it would be picked on isn't what it was aimed at.
pub const CLICK_REPUBLISHED: &str = "The click was let go: the model changed again before its \
                                     meshes were shown; click again once they are";

/// What the status bar says of a click let go as the read of the model's meshes was
/// refused: none are coming to pick it on.
pub const CLICK_UNREAD: &str = "The click was let go: the model's meshes couldn't be read";

/// What the status bar says of a click let go as no model is built to pick it on: none
/// yet, or the current revision's rebuild failed, or the gate refused the document.
pub const CLICK_NO_MODEL: &str = "The click was let go: the model isn't built";

/// The colours the selection and the hover are outlined in: [`theme`]'s, as plain values
/// for the renderer, which takes values, never the app's theme. SEAM: the prototype tints
/// a selected face ([`theme::VIEWPORT_SELECTION_TINT`]) and a hovered one
/// ([`theme::VIEWPORT_HOVER_TINT`]), and widens a selected edge; the renderer outlines both
/// at one width (re_renderer's outline has one radius), its edges' colours its own.
pub fn outlines() -> Outlines {
    let rgb = |[r, g, b, _]: theme::Rgba8| [r, g, b];
    Outlines {
        selected: rgb(theme::VIEWPORT_SELECTION_EDGE),
        hovered: rgb(theme::VIEWPORT_HOVER_EDGE),
    }
}

/// An item of the selection, in the order picked.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A feature: a row of the browser or an item of the timeline.
    Feature(FeatureId),
    /// A face or an edge, by name.
    Element(Picked),
}

/// A selected element: the name the selection holds for it, and what the published model
/// says of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Picked {
    /// As an agent writes it: the stable name the core holds for it, if it has one; else
    /// the name picked.
    pub name: String,
    pub state: State,
    /// How it's read: as picked, in the model it was picked in; or as carried into the
    /// model since published, until the next.
    carry: Carry,
    /// The model its answer ([`found`](Self::found)) is of: it's drawn only on bodies read
    /// from that model.
    found_in: Option<Of>,
}

/// What the published model says of a selected element.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Found: the core's answer, its stability, and the refusal a store would give if it
    /// isn't stable ([`Selected::stored`]).
    Found(Selected),
    /// Not answered: `why`, if its feature didn't build (the core's words for it), or none
    /// if its body's elements aren't named yet. The last answer it had, if it had one, is
    /// kept and drawn meanwhile, so the selection doesn't flicker.
    Waiting {
        why: Option<String>,
        last: Option<Selected>,
    },
}

impl Picked {
    /// The core's answer for it: its own, or, waiting, the last it had.
    pub fn found(&self) -> Option<&Selected> {
        match &self.state {
            State::Found(selected) => Some(selected),
            State::Waiting { last, .. } => last.as_ref(),
        }
    }

    /// Whether a store can take it: found, and its name stable.
    pub fn stable(&self) -> bool {
        matches!(&self.state, State::Found(selected) if selected.stable())
    }
}

/// What the pointer is over in the canvas: the body and element the renderer found, its
/// name as the element map gives it, whether that's stable, and the core's answer for it
/// (none if the published model doesn't find it, or hasn't named its body yet).
#[derive(Debug, Clone, PartialEq)]
pub struct Hovered {
    pub body: u32,
    pub element: Element,
    pub name: Arc<Name>,
    pub stable: bool,
    pub found: Option<Selected>,
}

impl Hovered {
    /// The feature that made it, as its name says (what the timeline outlines, DESIGN.md's
    /// *Hover linking*).
    pub fn feature(&self) -> &FeatureId {
        self.name.feature()
    }
}

/// What a pick does to the selection: a click replaces it, a Ctrl-click adds the element,
/// or removes it if it's selected; a box with Shift adds what's inside, selected or not
/// (SolidWorks' Box Selection, docs/ux/solidworks.md §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Replace,
    Toggle,
    Add,
}

/// Which kinds of element the canvas picks (SolidWorks' selection filters: SELECT's
/// buttons, and X, E, and V, `crate::shortcuts`), and a box takes (`crate::box_select`).
/// Vertices aren't drawn yet, so none is picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filter {
    pub faces: bool,
    pub edges: bool,
    pub vertices: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            faces: true,
            edges: true,
            vertices: true,
        }
    }
}

/// A click waiting for its pick's answer.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Click {
    rect: PixelRect,
    how: How,
    /// The newest frame that carried a pick before the click: its answer is to a frame
    /// after it.
    after: Option<u64>,
    /// The frames it has asked its pick in.
    frames: u32,
    /// The filter it was made under: it picks by it, or not at all.
    filter: Filter,
    /// When it was made, by the viewport's clock: [`CLICK_WAIT`] from then, it's let go.
    at: Instant,
    /// The published model whose meshes it's to be picked on, once that's known: the one
    /// published when it was made, if the view's meshes were of another then; else the
    /// first published after it. Another published before they come, it's let go.
    awaits: Option<Of>,
}

/// The left button pressed on the canvas: where, and whether it has gone farther than a
/// click's distance since.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Press {
    at_px: [f32; 2],
    far: bool,
}

/// The view a hover is of: the camera and the bodies table. Once either changes, what's
/// hovered is let go.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    camera: silico_protocol::Camera,
    table: u64,
}

/// Which published model: the tab's count of the models it has seen published, and its
/// revision (as [`View::bodies_of`] gives the bodies').
type Of = (u64, Revision);

/// The tab's selection and hover.
#[derive(Debug, Default)]
pub struct Selection {
    items: Vec<Item>,
    /// One more each time the items change: the highlights are worked out again by it.
    version: u64,
    /// What the status bar says of the last change a rebuild made (a name gone), or of a
    /// pick the model refused, until the selection changes again.
    said: Option<String>,
    hovered: Option<Hovered>,
    pub filter: Filter,
    /// The published model, and which it is, as the selection was last carried to it.
    model: Option<Arc<Model>>,
    of: Option<Of>,
    /// Whether some names were pending at the last read (asked again each frame).
    again: bool,
    /// The body row the browser scrolls to: a canvas pick's.
    scroll: Option<FeatureId>,
    press: Option<Press>,
    click: Option<Click>,
    /// The pointer's pixel the hover was last asked at.
    hover_asked: Option<[u32; 2]>,
    /// The view the hover is of, and the newest frame that carried a pick before it changed
    /// last: a hover's answer to that frame or an earlier one is of another view.
    seen: Option<Seen>,
    stale_to: Option<u64>,
    /// What a rebuild left gone, as said, and the model whose carry said it: a click let go
    /// as that model's meshes are awaited says nothing over it.
    gone_said: Option<(Of, String)>,
    /// The hover's answer taken this frame, if one was: the pixel it was asked at, and what
    /// it found there (M5.7's box select reads it, `crate::box_select`).
    heard: Option<([u32; 2], Option<Arc<Name>>)>,
    /// The newest picking answer taken, by its frame.
    taken: Option<u64>,
    /// The version and bodies table the highlights were last worked out for.
    drawn: Option<(u64, u64)>,
}

impl Selection {
    /// Its items, in the order picked.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The elements' names, in the order picked, as an agent writes them.
    pub fn names(&self) -> Vec<&str> {
        self.elements().map(|picked| picked.name.as_str()).collect()
    }

    /// The elements selected, in the order picked.
    pub fn elements(&self) -> impl Iterator<Item = &Picked> {
        self.items.iter().filter_map(|item| match item {
            Item::Element(picked) => Some(picked),
            Item::Feature(_) => None,
        })
    }

    /// The feature selected, if the selection is one feature and nothing else: a row of
    /// the browser or the timeline, whose properties show, H hides, and a sweep's dialog
    /// takes as its sketch.
    pub fn feature(&self) -> Option<&FeatureId> {
        match self.items.as_slice() {
            [Item::Feature(id)] => Some(id),
            _ => None,
        }
    }

    /// Whether feature `id` is selected as itself (its timeline item, its browser row).
    pub fn has_feature(&self, id: &FeatureId) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, Item::Feature(each) if each == id))
    }

    /// Whether the browser marks body `id`'s row: the body selected, or an element of it.
    pub fn marks_body(&self, id: &FeatureId) -> bool {
        self.has_feature(id)
            || self
                .elements()
                .filter_map(Picked::found)
                .any(|selected| &selected.feature == id)
    }

    /// What a store writes for each element selected, in the order picked: each found
    /// element's stable name (a fillet's `edges`); or, for the first that can't be stored,
    /// why, as a fillet's reference refuses it (`reference_broken`, the name unstable, with
    /// the stable names it most likely meant); or, for one waiting, that it isn't answered
    /// yet, or that its feature didn't build. Features aren't stored by name, and are left
    /// out.
    pub fn storable(&self) -> Result<Vec<Arc<Name>>, Error> {
        self.elements()
            .enumerate()
            .map(|(place, picked)| match &picked.state {
                State::Found(selected) => selected.stored.clone(),
                State::Waiting { why, .. } => {
                    let message = match why {
                        Some(why) => format!(
                            "{} isn't in the model as it's built now: {why}",
                            picked.name
                        ),
                        None => format!(
                            "{} isn't answered yet: its body's elements aren't named yet",
                            picked.name
                        ),
                    };
                    Err(Error::new(ErrorCode::Conflict, message)
                        .with_element(picked.name.as_str())
                        .with_parameter(format!("{PARAMETER}.{place}"))
                        .with_suggestion("select it again once the model has rebuilt"))
                }
            })
            .collect()
    }

    /// What the pointer is over in the canvas, if anything.
    pub fn hovered(&self) -> Option<&Hovered> {
        self.hovered.as_ref()
    }

    /// The feature that made what the pointer is over (the timeline outlines it, DESIGN.md's
    /// *Hover linking*).
    pub fn hovered_feature(&self) -> Option<&FeatureId> {
        self.hovered.as_ref().map(Hovered::feature)
    }

    /// The hover's answer taken this frame, if one was: the pixel it was asked at, and the
    /// name of what it found there, as the filter takes it (none: nothing). Never a click's
    /// answer, nor one of another view than the hover's (M5.7's box select).
    pub(crate) fn heard(&self) -> Option<&([u32; 2], Option<Arc<Name>>)> {
        self.heard.as_ref()
    }

    /// One more each time its items change (M5.7's box select lets a drag waiting go when
    /// it does, as a click waiting is let go).
    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    /// Which published model the selection is read against: the tab's count of them and
    /// its revision (M5.7's box select lets a press go when it changes).
    pub(crate) fn model_of(&self) -> Option<(u64, Revision)> {
        self.of
    }

    /// What the status bar says of the selection's last change, if anything: a name a
    /// rebuild left gone, or a pick the model didn't find.
    pub fn said(&self) -> Option<&str> {
        self.said.as_deref()
    }

    /// The body row the browser is to scroll to, once: a canvas pick's body.
    pub fn scroll_target(&self) -> Option<&FeatureId> {
        self.scroll.as_ref()
    }

    /// The browser has scrolled to its row.
    pub fn scrolled(&mut self) {
        self.scroll = None;
    }

    /// Feature `id` selected alone (a click on its row in the browser or the timeline). A
    /// click in the canvas waiting for its pick is let go: it came before.
    pub fn select_feature(&mut self, id: FeatureId) {
        if self.feature() != Some(&id) {
            self.set(vec![Item::Feature(id)]);
        }
        self.said = None;
        self.click = None;
    }

    /// Everything let go (Esc, a click on empty canvas, SELECT's Clear selection), a click
    /// waiting for its pick too, so its answer doesn't select after it.
    pub fn clear(&mut self) {
        if !self.items.is_empty() {
            self.set(Vec::new());
        }
        self.said = None;
        self.click = None;
    }

    /// `name` picked, `how` (a click's element, by its name as the element map gives it,
    /// or a name an agent could have written): read at once against the published model,
    /// and selected; or, if the model doesn't find it, the selection kept and why said.
    /// The rest are read again as each came to the model: a name carried into it stays
    /// carried, its unstable text gone, never re-pointed as a pick would hold it.
    pub fn pick(&mut self, name: &str, how: How) {
        let _span = tracing::debug_span!("selection_pick", ?how).entered();
        self.said = None;
        let Some(model) = self.model.clone() else {
            self.said = Some(format!("Can't select {name}: no model is built yet"));
            return;
        };
        let answer = match rules::resolve(&model, &[name], PARAMETER) {
            Ok(answer) => answer,
            Err(error) => {
                self.said = Some(format!("Can't select {name}: {}", error.message()));
                return;
            }
        };
        if let Some(gone) = answer.gone.first() {
            self.said = Some(format!("Can't select {name}: {}", words(&gone.why)));
            return;
        }
        let picked = match answer.kept.into_iter().next() {
            Some(selected) => Picked {
                name: selected.name.to_string(),
                state: State::Found(selected),
                carry: Carry::Pick,
                found_in: self.of,
            },
            None => Picked {
                name: name.to_owned(),
                state: State::Waiting {
                    why: None,
                    last: None,
                },
                carry: Carry::Pick,
                found_in: None,
            },
        };
        if let Some(found) = picked.found() {
            self.scroll = Some(found.feature.clone());
        }
        let mut items = match how {
            How::Replace => Vec::new(),
            How::Toggle | How::Add => self.items.clone(),
        };
        match items.iter().position(|item| same(item, &picked)) {
            Some(at) if how == How::Toggle => {
                items.remove(at);
                self.scroll = None;
            }
            // Added, and selected already: kept where it was.
            Some(_) => {}
            None => items.push(Item::Element(picked)),
        }
        self.set(items);
        self.read(&model);
    }

    /// `names` picked together, `how` (a box's elements, by their names as the element map
    /// gives them): read at once against the published model in one read, and the
    /// selection changed once, so it's one change, one notice, and one revision of the
    /// items, however many names. Replace makes the selection the names found (none: it's
    /// cleared, as [`clear`](Self::clear) clears it); Add adds each found that isn't
    /// selected; Toggle removes each found that is and adds the rest. A name the model
    /// doesn't find is left out, and the first is said, with how many; one waiting (its body's elements not named yet) is added
    /// waiting, as a click's is. The same element named twice is taken once. A selection
    /// that would hold more elements than the core reads at once
    /// ([`rules::MAX_SELECTED`]) isn't made: the selection is kept and that's said.
    pub fn pick_all(&mut self, names: &[String], how: How) {
        let _span = tracing::debug_span!("selection_pick_all", ?how, names = names.len()).entered();
        self.said = None;
        if names.is_empty() {
            if how == How::Replace {
                self.clear();
            }
            return;
        }
        let Some(model) = self.model.clone() else {
            self.said = Some(format!(
                "Can't select {}: no model is built yet",
                count_of(names.len())
            ));
            return;
        };
        if names.len() > rules::MAX_SELECTED {
            self.said = Some(too_many(names.len()));
            return;
        }
        let answer = match rules::resolve(&model, names, PARAMETER) {
            Ok(answer) => answer,
            Err(error) => {
                self.said = Some(format!(
                    "Can't select {}: {}",
                    count_of(names.len()),
                    error.message()
                ));
                return;
            }
        };
        if let Some(first) = answer.gone.first() {
            let why = words(&first.why);
            self.said = Some(match answer.gone.len() {
                1 => format!("Can't select {}: {why}", first.name),
                many => format!(
                    "Can't select {many} of the {}: {why}",
                    count_of(names.len())
                ),
            });
        }
        // In the order named: each found, by the name the core holds it by; each pending,
        // waiting.
        let mut each: BTreeMap<usize, Picked> = BTreeMap::new();
        for selected in answer.kept {
            each.insert(
                selected.place,
                Picked {
                    name: selected.name.to_string(),
                    state: State::Found(selected),
                    carry: Carry::Pick,
                    found_in: self.of,
                },
            );
        }
        for pending in answer.pending {
            each.insert(
                pending.place,
                Picked {
                    name: pending.name,
                    state: State::Waiting {
                        why: None,
                        last: None,
                    },
                    carry: Carry::Pick,
                    found_in: None,
                },
            );
        }
        let mut items = match how {
            How::Replace => Vec::new(),
            How::Toggle | How::Add => self.items.clone(),
        };
        // Each picked matched to the item that's its element as [`same`] matches them, by
        // index, so a box of thousands costs no more than a sort: the items', and those the
        // batch has added so far, so the same element named twice is taken once.
        let mut index = Index::default();
        for (at, item) in items.iter().enumerate() {
            if let Item::Element(picked) = item {
                index.add(at, picked);
            }
        }
        let mut removed: BTreeSet<usize> = BTreeSet::new();
        let mut taken: BTreeSet<usize> = BTreeSet::new();
        let mut scroll = None;
        for picked in each.into_values() {
            match index.find(&picked) {
                // Named twice in the batch: taken once.
                Some(at) if taken.contains(&at) => {}
                Some(at) => {
                    if how == How::Toggle {
                        removed.insert(at);
                    }
                    taken.insert(at);
                }
                None => {
                    let at = items.len();
                    index.add(at, &picked);
                    taken.insert(at);
                    if scroll.is_none() {
                        scroll = picked.found().map(|found| found.feature.clone());
                    }
                    items.push(Item::Element(picked));
                }
            }
        }
        let items: Vec<Item> = items
            .into_iter()
            .enumerate()
            .filter(|(at, _)| !removed.contains(at))
            .map(|(_, item)| item)
            .collect();
        let elements = items
            .iter()
            .filter(|item| matches!(item, Item::Element(_)))
            .count();
        if elements > rules::MAX_SELECTED {
            self.said = Some(too_many(elements));
            return;
        }
        if items != self.items {
            self.scroll = scroll;
            // One revision, whether the read makes the items again (each answer's place
            // is its place in the selection, not in `names`) or not.
            let version = self.version;
            self.items = items;
            self.read(&model);
            if self.version == version {
                self.version = version.wrapping_add(1);
            }
        }
    }

    /// The model the session published, which it is (its revision), and `models`, the tab's
    /// count of them (`Tab::published_models`), each frame: the selection carried to a new
    /// one (the core's refresh: names kept stay selected, with their stability now; names
    /// gone are dropped, and said, the rest kept; names of a feature that didn't build
    /// wait), each name carried until the next; and names pending asked again of the same,
    /// each as it came to it.
    pub(crate) fn published(&mut self, published: Option<(Arc<Model>, Revision)>, models: u64) {
        let Some((model, revision)) = published else {
            self.model = None;
            return;
        };
        if self.of != Some((models, revision)) {
            self.of = Some((models, revision));
            self.model = Some(Arc::clone(&model));
            // A click waiting for the meshes of a model published before this one: they're
            // not what it would be picked on now. Said before the carry, whose word of what's
            // gone, if any, is said over it.
            if let Some(click) = &mut self.click {
                match click.awaits {
                    Some(awaited) if awaited != (models, revision) => {
                        tracing::debug!("another model published under a click waiting: let go");
                        self.click = None;
                        self.said = Some(CLICK_REPUBLISHED.to_owned());
                    }
                    // Its frames asked on the previous meshes don't count against its
                    // wait for an answer on these.
                    _ => {
                        click.awaits = Some((models, revision));
                        click.frames = 0;
                    }
                }
            }
            for item in &mut self.items {
                if let Item::Element(picked) = item {
                    picked.carry = Carry::Refresh;
                }
            }
            if self.elements().next().is_some() {
                self.read(&model);
            }
        } else if self.again {
            self.read(&model);
        }
    }

    /// Every element name read against `model` in one read, each as it came to the model
    /// ([`rules::read_each`]: each error's place is the name's place in the selection, and
    /// an element is taken once, whichever way its names came), and the items made again
    /// from the answer.
    fn read(&mut self, model: &Model) {
        let names: Vec<(String, Carry)> = self
            .elements()
            .map(|picked| (picked.name.clone(), picked.carry))
            .collect();
        if names.is_empty() {
            self.again = false;
            return;
        }
        match rules::read_each(model, &names, PARAMETER) {
            Ok(answer) => self.took(answer),
            Err(error) => {
                self.again = false;
                self.said = Some(error.message().to_owned());
            }
        }
    }

    /// The items made again from the read of every element's name, in order: each name
    /// kept found (held by the name the core holds it by), each pending waiting (asked again
    /// next frame), each of a feature that didn't build waiting for the next model, each gone
    /// dropped (the rest kept) and said, and a name read once already (a repeat, or another
    /// name of an element an earlier one found) dropped, unsaid and uncounted: it was the
    /// same element as another.
    fn took(&mut self, answer: Resolved) {
        self.again = false;
        let mut kept: BTreeMap<usize, Selected> =
            answer.kept.into_iter().map(|s| (s.place, s)).collect();
        let pending: BTreeSet<usize> = answer.pending.iter().map(|p| p.place).collect();
        let mut gone: BTreeMap<usize, Gone> =
            answer.gone.into_iter().map(|g| (g.place, g)).collect();
        // The selection before, by kind as each was found, for what's said of what's gone:
        // each item but those dropped as another's element.
        let mut before: BTreeMap<Option<ElementKind>, usize> = BTreeMap::new();
        let mut dropped: Vec<(Option<ElementKind>, Gone)> = Vec::new();
        let mut found: BTreeSet<(FeatureId, ElementRef)> = BTreeSet::new();
        let mut items = Vec::with_capacity(self.items.len());
        let mut place = 0usize;
        for item in std::mem::take(&mut self.items) {
            let picked = match item {
                Item::Feature(id) => {
                    items.push(Item::Feature(id));
                    continue;
                }
                Item::Element(picked) => picked,
            };
            let at = place;
            place += 1;
            let last = picked.found().cloned();
            let kind = last.as_ref().map(Selected::kind);
            let mut count = || *before.entry(kind).or_default() += 1;
            if let Some(selected) = kept.remove(&at) {
                // An element another name found already: taken once, at the first. The core
                // takes each once in its read; this holds to it whatever it answers.
                if found.insert((selected.feature.clone(), selected.element)) {
                    count();
                    items.push(Item::Element(Picked {
                        name: selected.name.to_string(),
                        state: State::Found(selected),
                        carry: picked.carry,
                        found_in: self.of,
                    }));
                }
            } else if pending.contains(&at) {
                count();
                self.again = true;
                items.push(Item::Element(Picked {
                    name: picked.name,
                    state: State::Waiting { why: None, last },
                    ..picked
                }));
            } else if let Some(gone) = gone.remove(&at) {
                count();
                if gone.failed.is_some() {
                    items.push(Item::Element(Picked {
                        name: picked.name,
                        state: State::Waiting {
                            why: Some(words(&gone.why)),
                            last,
                        },
                        ..picked
                    }));
                } else {
                    dropped.push((kind, gone));
                }
            }
        }
        if let Some((_, first)) = dropped.first() {
            let said = gone_words(&dropped, &before);
            self.gone_said = self.of.map(|of| (of, said.clone()));
            self.said = Some(said);
            tracing::info!(gone = dropped.len(), first = %first.name, "selection_gone");
        }
        if items != self.items {
            self.set(items);
        }
    }

    fn set(&mut self, items: Vec<Item>) {
        self.items = items;
        self.version = self.version.wrapping_add(1);
    }

    /// The canvas's left button and pointer this frame (`frame`, given to the tab's
    /// overlay): a click taken, its pick asked of `view` until it's answered, and the
    /// hover asked and taken; and the highlights and the hover set on `view` to draw.
    /// `ctx` gives egui's click distance. While `view`'s bodies are of another model than
    /// the selection's, nothing is named from them: a click waits for the meshes, asking
    /// nothing, and the hover is let go.
    pub(crate) fn overlay(&mut self, view: &mut View, frame: &ViewFrame, ctx: &egui::Context) {
        let ppp = frame.pixels_per_point;
        let click_px = ctx.options(|options| options.input_options.max_click_dist) * ppp;
        let band = band_px(ppp);
        let current = self.of.is_some() && view.bodies_of() == self.of;
        self.heard = None;
        if let Some((at_px, how)) = self.take_left(frame, click_px) {
            self.click = Some(Click {
                rect: square(at_px, frame.size_px, band),
                how,
                after: view.last_asked(),
                frames: 0,
                filter: self.filter,
                at: frame.now,
                awaits: if current { None } else { self.of },
            });
        }
        self.view_changed(view, frame);
        self.let_go_if_vain(view, frame.now, current);
        if let Some(picking) = view.picked()
            && self.taken.is_none_or(|taken| picking.frame > taken)
        {
            self.taken = Some(picking.frame);
            match self.click {
                // Of meshes of another model: the click waits, and asks again once they've
                // caught up.
                Some(_) if !current => {}
                Some(click) if click.after.is_none_or(|after| picking.frame > after) => {
                    self.click = None;
                    self.clicked(&picking, click.how, view.bodies());
                }
                // An answer to a pick asked before the click: neither the click's nor, now,
                // the hover's.
                Some(_) => {}
                None if current && self.stale_to.is_none_or(|stale| picking.frame > stale) => {
                    self.hover_answered(&picking, view.bodies());
                }
                None => {}
            }
        }
        self.ask(view, frame, band, current);
        if let Some(click) = &self.click
            && !current
        {
            // Nothing is counted while it waits for the meshes: their read's answer wakes
            // the app (the core's waker), and so does another model published. A frame is
            // asked for only at the backstop.
            let waited = frame.now.saturating_duration_since(click.at);
            ctx.request_repaint_after(CLICK_WAIT.saturating_sub(waited));
        }
        self.draw(view);
    }

    /// A click waiting let go if what it would pick by has changed (the filter), or, while
    /// the view's bodies aren't `current` (of the selection's model), if no meshes of that
    /// model are coming (no model built, or their read refused: said, in its cause's
    /// words), or once it has waited [`CLICK_WAIT`] (said). A click let go here asks
    /// nothing more.
    fn let_go_if_vain(&mut self, view: &View, now: Instant, current: bool) {
        let Some(click) = &self.click else {
            return;
        };
        if click.filter != self.filter {
            tracing::debug!("the filter changed under a click waiting: let go");
            self.click = None;
            return;
        }
        if current {
            return;
        }
        let said = if self.model.is_none() {
            CLICK_NO_MODEL
        } else if self
            .of
            .is_some_and(|(_, revision)| view.refused_of(revision))
        {
            CLICK_UNREAD
        } else if now.saturating_duration_since(click.at) >= CLICK_WAIT {
            CLICK_STALE
        } else {
            return;
        };
        tracing::debug!(said, "a click waiting for the model's meshes: let go");
        self.click = None;
        self.say_let_go(said);
    }

    /// `said` of a click let go, unless what's said is what a rebuild left gone from the
    /// model the click waited for: that's said, and stays.
    fn say_let_go(&mut self, said: &str) {
        let gone_now = match (&self.gone_said, self.of) {
            (Some((of, gone)), Some(now)) => *of == now && self.said.as_ref() == Some(gone),
            _ => false,
        };
        if !gone_now {
            self.said = Some(said.to_owned());
        }
    }

    /// The canvas claimed by a mode that takes its clicks and pointer (sketch mode,
    /// SEAM(#351)), for as long as it holds it: a click waiting for its pick let go (it was
    /// the selection's, and is answered on a canvas the mode now draws), the press and the
    /// hover too.
    pub fn canvas_claimed(&mut self) {
        self.click = None;
        self.press = None;
        self.hovered = None;
        self.hover_asked = None;
    }

    /// What's hovered let go if the camera or the bodies changed since the last frame: the
    /// pointer may be over something else now. It's asked again when the pointer moves. A
    /// click waiting for its pick is let go if the camera changed: its pixel was of the view
    /// before. (Not for the bodies alone: a click waiting for the meshes of the selection's
    /// model is asked once they come.)
    fn view_changed(&mut self, view: &View, frame: &ViewFrame) {
        let now = Seen {
            camera: frame.camera.clone(),
            table: view.table(),
        };
        if self.seen.as_ref() != Some(&now) {
            if let Some(seen) = &self.seen {
                self.hovered = None;
                self.stale_to = view.last_asked();
                if seen.camera != now.camera && self.click.take().is_some() {
                    tracing::debug!("the camera moved under a click waiting for its pick: let go");
                }
            }
            self.seen = Some(now);
        }
    }

    /// The left button's events this frame: a press and its release within `click_px` of
    /// it (physical pixels), with no cancel between, is a click, at the press, Ctrl held
    /// at the release toggling; the last of the frame's, if it has several.
    fn take_left(&mut self, frame: &ViewFrame, click_px: f32) -> Option<([f32; 2], How)> {
        let mut click = None;
        for event in &frame.left {
            match event.kind {
                LeftKind::Press => {
                    self.press = Some(Press {
                        at_px: event.at_px,
                        far: false,
                    });
                }
                LeftKind::Move => {
                    if let Some(press) = &mut self.press {
                        press.far |= distance(press.at_px, event.at_px) > click_px;
                    }
                }
                LeftKind::Release => {
                    if let Some(press) = self.press.take()
                        && !press.far
                        && distance(press.at_px, event.at_px) <= click_px
                    {
                        let how = if event.ctrl {
                            How::Toggle
                        } else {
                            How::Replace
                        };
                        click = Some((press.at_px, how));
                    }
                }
                LeftKind::Cancel => self.press = None,
                LeftKind::DoubleClick => {}
            }
        }
        click
    }

    /// The pick asked this frame: the click's, again, while one waits; else the hover's, if
    /// the pointer has moved to another pixel since it was last asked, and no drag
    /// navigates the view. Neither while the view's bodies aren't `current` (of the
    /// selection's model): a click waits, asking nothing and counting no frames
    /// ([`let_go_if_vain`](Self::let_go_if_vain) says when it's let go); and the hover is
    /// let go.
    fn ask(&mut self, view: &mut View, frame: &ViewFrame, band: u32, current: bool) {
        if !current {
            self.hovered = None;
            self.hover_asked = None;
            return;
        }
        if let Some(click) = &mut self.click {
            click.frames += 1;
            if click.frames > CLICK_FRAMES {
                tracing::debug!("a click's pick never answered: let go");
                self.click = None;
            } else {
                let rect = click.rect;
                view.pick(rect);
                // The hover is asked again once the click has its answer.
                self.hover_asked = None;
                return;
            }
        }
        let Some(pointer) = frame.pointer_px else {
            self.hovered = None;
            self.hover_asked = None;
            return;
        };
        if view.navigating() {
            self.hovered = None;
            return;
        }
        let rect = square(pointer, frame.size_px, band);
        let pixel = centre_of(rect);
        if self.hover_asked != Some(pixel) && view.pick(rect) {
            self.hover_asked = Some(pixel);
        }
    }

    /// A click's pick answered: what's under it selected `how`; nothing there, the
    /// selection cleared by a plain click, kept by a Ctrl-click.
    fn clicked(&mut self, picking: &Picking, how: How, bodies: &BTreeMap<u32, ViewBody>) {
        match self.under(picking, bodies) {
            Some((_, _, name, _)) => self.pick(&name.to_string(), how),
            None if how == How::Replace => self.clear(),
            None => {}
        }
    }

    /// A hover's pick answered: what's under the pointer, read against the published model
    /// when it's another element than before.
    fn hover_answered(&mut self, picking: &Picking, bodies: &BTreeMap<u32, ViewBody>) {
        let under = self.under(picking, bodies);
        self.heard = Some((
            centre_of(picking.rect),
            under.as_ref().map(|(_, _, name, _)| Arc::clone(name)),
        ));
        let Some((body, element, name, stable)) = under else {
            self.hovered = None;
            return;
        };
        if self
            .hovered
            .as_ref()
            .is_some_and(|hovered| (hovered.body, hovered.element) == (body, element))
        {
            return;
        }
        let found = self.model.as_ref().and_then(|model| {
            rules::resolve(model, &[name.to_string()], PARAMETER)
                .ok()
                .and_then(|answer| answer.kept.into_iter().next())
        });
        self.hovered = Some(Hovered {
            body,
            element,
            name,
            stable,
            found,
        });
    }

    /// What a picking found under its centre, as the filter takes it: an edge in its
    /// square, first; else the face at its centre pixel. Its body, element, name, and the
    /// name's stability, as the bodies' meshes name them.
    fn under(
        &self,
        picking: &Picking,
        bodies: &BTreeMap<u32, ViewBody>,
    ) -> Option<(u32, Element, Arc<Name>, bool)> {
        let centre = centre_of(picking.rect);
        let edge = picking.edge.filter(|_| self.filter.edges);
        let face = picking.hit.filter(|hit| {
            hit.pixel == centre && matches!(hit.hit, Element::Face(_)) && self.filter.faces
        });
        let pick = edge.or(face)?;
        let named = bodies.get(&pick.body)?.name_of(pick.hit)?;
        Some((pick.body, pick.hit, Arc::clone(&named.name), named.stable))
    }

    /// The highlights and the hover set on `view`: the highlights worked out again only
    /// when the selection or the bodies table changed (which model the bodies are of
    /// changes only with the table).
    fn draw(&mut self, view: &mut View) {
        let key = (self.version, view.table());
        if self.drawn != Some(key) {
            view.highlights = highlights(&self.items, view.bodies(), view.bodies_of());
            self.drawn = Some(key);
        }
        view.hovered = self
            .hovered
            .iter()
            .map(|hovered| (hovered.body, hovered.element))
            .collect();
    }
}

/// What the canvas highlights of `items` among `bodies`, read from the model `of`: each
/// element found, in the body it's in (a vertex isn't drawn), if the answer it's drawn by
/// is of that model (an element's number in another model may be another element of these
/// meshes); each feature's faces, a body's whole (its browser row), else those it made, as
/// each face's name says (as `View::linked` lights a feature hovered in the timeline).
fn highlights(
    items: &[Item],
    bodies: &BTreeMap<u32, ViewBody>,
    of: Option<Of>,
) -> BTreeSet<(u32, Element)> {
    let mut lit = BTreeSet::new();
    let body_of = |feature: &FeatureId| {
        bodies
            .iter()
            .find(|(_, body)| &body.feature == feature)
            .map(|(number, _)| *number)
    };
    for item in items {
        match item {
            Item::Element(picked) if of.is_none() || picked.found_in != of => {}
            Item::Element(picked) => {
                let Some(selected) = picked.found() else {
                    continue;
                };
                let Some(number) = body_of(&selected.feature) else {
                    continue;
                };
                if let Some(element) = drawn_as(selected.element) {
                    lit.insert((number, element));
                }
            }
            Item::Feature(id) => {
                for (number, body) in bodies {
                    let whole = &body.feature == id;
                    let faces = body.faces.iter().zip(1u32..);
                    lit.extend(
                        faces
                            .filter(|(face, _)| whole || face.name.feature() == id)
                            .map(|(_, n)| (*number, Element::Face(n))),
                    );
                }
            }
        }
    }
    lit
}

/// An element as the renderer numbers it: a face or an edge by its number from 1; none for
/// a vertex or a solid, which it doesn't draw.
fn drawn_as(element: ElementRef) -> Option<Element> {
    match element.kind {
        ElementKind::Face => Some(Element::Face(element.index)),
        ElementKind::Edge => Some(Element::Edge(element.index)),
        ElementKind::Vertex | ElementKind::Solid => None,
    }
}

/// The elements of a list of items by where each stands, to find a pick's as [`same`]
/// matches them without a scan: a found pick by its element among the found, else by its
/// name among those not found; one not found by its name among all.
#[derive(Default)]
struct Index {
    found: BTreeMap<(FeatureId, ElementRef), usize>,
    unfound: BTreeMap<String, usize>,
    named: BTreeMap<String, usize>,
}

impl Index {
    /// `picked`, standing `at`: the first of each key kept.
    fn add(&mut self, at: usize, picked: &Picked) {
        match picked.found() {
            Some(found) => {
                self.found
                    .entry((found.feature.clone(), found.element))
                    .or_insert(at);
            }
            None => {
                self.unfound.entry(picked.name.clone()).or_insert(at);
            }
        }
        self.named.entry(picked.name.clone()).or_insert(at);
    }

    /// Where `picked`'s element stands, if it's there.
    fn find(&self, picked: &Picked) -> Option<usize> {
        match picked.found() {
            Some(found) => self
                .found
                .get(&(found.feature.clone(), found.element))
                .or_else(|| self.unfound.get(&picked.name)),
            None => self.named.get(&picked.name),
        }
        .copied()
    }
}

/// Whether `item` is `picked`'s element: the same element found, or, either not found, the
/// same name.
fn same(item: &Item, picked: &Picked) -> bool {
    match item {
        Item::Element(each) => match (each.found(), picked.found()) {
            (Some(a), Some(b)) => a.feature == b.feature && a.element == b.element,
            _ => each.name == picked.name,
        },
        Item::Feature(_) => false,
    }
}

/// `count` elements, in words: "1 element", "3 elements".
fn count_of(count: usize) -> String {
    if count == 1 {
        "1 element".to_owned()
    } else {
        format!("{count} elements")
    }
}

/// What's said of a selection that would hold `count` elements, past what the core reads
/// at once.
fn too_many(count: usize) -> String {
    format!(
        "Can't select {count} elements: a selection holds at most {}; select fewer, or use a \
         filter",
        rules::MAX_SELECTED
    )
}

/// What a refusal's errors say, its first's message and suggestion (as the tab says a
/// refusal).
fn words(problems: &Problems) -> String {
    crate::tab::said(problems)
}

/// What the status bar says of the names a rebuild left gone, `dropped`, each with its
/// kind as it was found before (none if it wasn't): how many of each kind of the selection
/// before (`before`, by kind) went, and, for one, why, in the core's words. "2 of 12
/// selected edges are gone after the rebuild"; "1 of 2 selected faces is gone after the
/// rebuild: …"; "The selected face is gone after the rebuild: …". Faces first, then edges,
/// vertices, and elements of a kind not known (never found).
fn gone_words(
    dropped: &[(Option<ElementKind>, Gone)],
    before: &BTreeMap<Option<ElementKind>, usize>,
) -> String {
    let rank = |kind: Option<ElementKind>| match kind {
        Some(ElementKind::Face) => 0,
        Some(ElementKind::Edge) => 1,
        Some(ElementKind::Vertex) => 2,
        Some(ElementKind::Solid) => 3,
        None => 4,
    };
    let mut gone: BTreeMap<(u8, Option<ElementKind>), usize> = BTreeMap::new();
    for (kind, _) in dropped {
        *gone.entry((rank(*kind), *kind)).or_default() += 1;
    }
    let parts: Vec<String> = gone
        .iter()
        .map(|((_, kind), count)| {
            let of = before.get(kind).copied().unwrap_or(0).max(*count);
            let (one, many) =
                kind.map_or(("element", "elements"), |kind| (kind.word(), kind.plural()));
            if of == 1 {
                format!("the selected {one}")
            } else {
                format!("{count} of {of} selected {many}")
            }
        })
        .collect();
    let what = match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let verb = if dropped.len() == 1 { "is" } else { "are" };
    let mut said = format!("{what} {verb} gone after the rebuild");
    if let [(_, one)] = dropped {
        said = format!("{said}: {}", words(&one.why));
    }
    let mut chars = said.chars();
    chars.next().map_or(String::new(), |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The edge band, in physical pixels, at `ppp` pixels a point: at least one.
pub(crate) fn band_px(ppp: f32) -> u32 {
    // At most MAX_PIXELS_PER_POINT (32) × 3 points: well inside a u32, and the square
    // (2 · band + 1) inside the most a picking takes on a side (256).
    (EDGE_BAND_POINTS * ppp).ceil().clamp(1.0, 96.0) as u32
}

/// The square `band` pixels out on each side of the pixel `at_px` (physical pixels from
/// the viewport's top left, maybe outside it) is in, the pixel clamped into the viewport,
/// `size` pixels.
pub(crate) fn square(at_px: [f32; 2], size: [u32; 2], band: u32) -> PixelRect {
    let pixel = |at: f32, side: u32| {
        let last = side.saturating_sub(1);
        // Floored and clamped into [0, last]: a NaN is 0, as `as` takes it.
        (at.floor().max(0.0) as u32).min(last)
    };
    let centre = [pixel(at_px[0], size[0]), pixel(at_px[1], size[1])];
    // A viewport's side is a few thousand pixels: inside an i32.
    let corner = |c: u32| i32::try_from(c).unwrap_or(i32::MAX) - band as i32;
    PixelRect {
        min: [corner(centre[0]), corner(centre[1])],
        size: [2 * band + 1, 2 * band + 1],
    }
}

/// A square's centre pixel ([`square`]'s sides are odd).
pub(crate) fn centre_of(rect: PixelRect) -> [u32; 2] {
    let at = |min: i32, side: u32| u32::try_from(i64::from(min) + i64::from(side / 2)).unwrap_or(0);
    [at(rect.min[0], rect.size[0]), at(rect.min[1], rect.size[1])]
}

pub(crate) fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_core::command::meshes::MeshElement;
    use silico_core::naming::{PrimitiveKind, TypeWord};
    use silico_core::recompute::Key;
    use silico_core::selection::Pending;

    fn fid(id: &str) -> FeatureId {
        FeatureId::new(id).unwrap()
    }

    fn face(index: u32) -> ElementRef {
        ElementRef {
            kind: ElementKind::Face,
            index,
        }
    }

    /// Box `feature`'s `element`, by its name (`k7f2qx3m.box.Face3`).
    fn named(feature: &str, element: ElementRef) -> Arc<Name> {
        Arc::new(Name::Element {
            feature: fid(feature),
            made: TypeWord::Primitive(PrimitiveKind::Box),
            element,
        })
    }

    fn found(feature: &str, element: ElementRef, place: usize) -> Selected {
        let name = named(feature, element);
        Selected {
            name: Arc::clone(&name),
            place,
            feature: fid(feature),
            element,
            stored: Ok(name),
        }
    }

    fn picked(name: &str) -> Item {
        Item::Element(Picked {
            name: name.to_owned(),
            state: State::Waiting {
                why: None,
                last: None,
            },
            carry: Carry::Pick,
            found_in: None,
        })
    }

    /// `answer` taken as a read of the selection's names gives it.
    fn took(selection: &mut Selection, answer: Resolved) {
        selection.took(answer);
    }

    fn gone(name: &str, place: usize, failed: Option<&str>) -> Gone {
        Gone {
            name: name.to_owned(),
            place,
            why: Problems::from(Error::new(
                ErrorCode::ReferenceBroken,
                format!("{name} names nothing"),
            )),
            failed: failed.map(fid),
        }
    }

    fn resolved(kept: Vec<Selected>, gone: Vec<Gone>, pending: &[(usize, &str)]) -> Resolved {
        Resolved {
            kept,
            gone,
            pending: pending
                .iter()
                .map(|&(place, name)| Pending {
                    name: name.to_owned(),
                    place,
                })
                .collect(),
            failures: BTreeMap::new(),
        }
    }

    fn states(selection: &Selection) -> Vec<(String, &'static str)> {
        selection
            .elements()
            .map(|picked| {
                let state = match &picked.state {
                    State::Found(_) => "found",
                    State::Waiting { why: None, .. } => "pending",
                    State::Waiting { why: Some(_), .. } => "failed",
                };
                (picked.name.clone(), state)
            })
            .collect()
    }

    #[test]
    fn a_read_keeps_whats_found_waits_on_whats_pending_or_failed_and_drops_and_says_whats_gone() {
        // Five names and a feature between them, read at places 0 to 4 (the feature has
        // none): Face1 found; Face2 pending; Face3 of a feature that failed; Face4 gone;
        // Face1 again, which the core read once already, so it's in no list: dropped, and
        // not counted among the selection's elements in what's said (it was Face1's).
        let mut selection = Selection::default();
        let names = [
            "k7f2qx3m.box.Face1",
            "k7f2qx3m.box.Face2",
            "k7f2qx3m.box.Face3",
            "k7f2qx3m.box.Face4",
        ];
        selection.set(vec![
            picked(names[0]),
            Item::Feature(fid("a91cw0de")),
            picked(names[1]),
            picked(names[2]),
            picked(names[3]),
            picked(names[0]),
        ]);
        let before = selection.version;
        took(
            &mut selection,
            resolved(
                vec![found("k7f2qx3m", face(1), 0)],
                vec![gone(names[2], 2, Some("k7f2qx3m")), gone(names[3], 3, None)],
                &[(1, names[1])],
            ),
        );
        assert_eq!(
            states(&selection),
            [
                (names[0].to_owned(), "found"),
                (names[1].to_owned(), "pending"),
                (names[2].to_owned(), "failed"),
            ]
        );
        assert_eq!(selection.items()[1], Item::Feature(fid("a91cw0de")));
        assert!(selection.again, "a pending name isn't asked again");
        assert_ne!(selection.version, before);
        assert_eq!(
            selection.said(),
            Some(
                "1 of 4 selected elements is gone after the rebuild: k7f2qx3m.box.Face4 names \
                 nothing"
            )
        );
        // The failed one says why, in the core's words.
        let State::Waiting { why, .. } = &selection.elements().nth(2).unwrap().state else {
            panic!()
        };
        assert_eq!(why.as_deref(), Some("k7f2qx3m.box.Face3 names nothing"));
    }

    #[test]
    fn a_name_waiting_keeps_its_last_answer_to_draw_and_found_again_is_held_by_the_cores_name() {
        // Face1 found, then pending: its last answer kept, so it's still drawn; then found
        // again, held by the name the core holds it by (here its own).
        let mut selection = Selection::default();
        selection.set(vec![picked("k7f2qx3m.box.Face1")]);
        took(
            &mut selection,
            resolved(vec![found("k7f2qx3m", face(1), 0)], vec![], &[]),
        );
        assert!(!selection.again);
        took(
            &mut selection,
            resolved(vec![], vec![], &[(0, "k7f2qx3m.box.Face1")]),
        );
        let picked = selection.elements().next().unwrap();
        assert_eq!(picked.found(), Some(&found("k7f2qx3m", face(1), 0)));
        assert!(!picked.stable(), "a waiting name is storable");
        assert!(selection.again);
        took(
            &mut selection,
            resolved(vec![found("k7f2qx3m", face(1), 0)], vec![], &[]),
        );
        assert_eq!(
            states(&selection),
            [("k7f2qx3m.box.Face1".to_owned(), "found")]
        );
        assert!(!selection.again);
        assert_eq!(selection.said(), None);
    }

    #[test]
    fn storable_gives_each_stable_name_in_order_or_the_first_refusal() {
        let mut selection = Selection::default();
        selection.set(vec![
            Item::Feature(fid("a91cw0de")),
            picked("k7f2qx3m.box.Face1"),
            picked("k7f2qx3m.box.Face2"),
        ]);
        took(
            &mut selection,
            resolved(
                vec![found("k7f2qx3m", face(1), 0), found("k7f2qx3m", face(2), 1)],
                vec![],
                &[],
            ),
        );
        let stored: Vec<String> = selection
            .storable()
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(stored, ["k7f2qx3m.box.Face1", "k7f2qx3m.box.Face2"]);
        // The second pending: refused, at its place among the elements, naming it.
        took(
            &mut selection,
            resolved(
                vec![found("k7f2qx3m", face(1), 0)],
                vec![],
                &[(1, "k7f2qx3m.box.Face2")],
            ),
        );
        let refusal = selection.storable().unwrap_err();
        assert_eq!(refusal.code(), ErrorCode::Conflict);
        assert_eq!(refusal.parameter(), Some("selection.1"));
        assert_eq!(refusal.elements(), ["k7f2qx3m.box.Face2"]);
        assert_eq!(
            refusal.message(),
            "k7f2qx3m.box.Face2 isn't answered yet: its body's elements aren't named yet"
        );
    }

    fn event(kind: LeftKind, at_px: [f32; 2], ctrl: bool) -> crate::viewport::LeftEvent {
        crate::viewport::LeftEvent {
            kind,
            at_px,
            shift: false,
            ctrl,
            frame: 0,
            time: 0.0,
            seq: 0,
        }
    }

    fn frame_of(left: Vec<crate::viewport::LeftEvent>) -> ViewFrame {
        ViewFrame {
            session: silico_core::command::SessionId::parse("s1").unwrap(),
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0)),
            camera: crate::navigate::HOME.camera(crate::navigate::NO_BOX, 100, 100),
            size_px: [100, 100],
            pixels_per_point: 1.0,
            pointer_px: None,
            left,
            secondary: Vec::new(),
            now: Instant::now(),
        }
    }

    #[test]
    fn a_press_and_its_release_within_the_click_distance_is_a_click_at_the_press() {
        use LeftKind::{Cancel, Move, Press, Release};
        let mut selection = Selection::default();
        let take = |selection: &mut Selection, left| selection.take_left(&frame_of(left), 6.0);
        // Moved 6 pixels away and back: a click, at the press, Ctrl at the release
        // toggling.
        let click = take(
            &mut selection,
            vec![
                event(Press, [10.0, 10.0], false),
                event(Move, [16.0, 10.0], false),
                event(Release, [11.0, 10.0], true),
            ],
        );
        assert_eq!(click, Some(([10.0, 10.0], How::Toggle)));
        // Past 6 at a move, though let go at the press: a drag, no click.
        let dragged = take(
            &mut selection,
            vec![
                event(Press, [10.0, 10.0], false),
                event(Move, [10.0, 16.5], false),
                event(Release, [10.0, 10.0], false),
            ],
        );
        assert_eq!(dragged, None);
        // Let go past 6 with no move between: no click either.
        let far = vec![
            event(Press, [0.0, 0.0], false),
            event(Release, [4.0, 4.5], false),
        ];
        assert_eq!(take(&mut selection, far), None);
        // A press, then a frame later its release: a click, plain.
        assert_eq!(
            take(&mut selection, vec![event(Press, [3.0, 4.0], false)]),
            None
        );
        let release = vec![event(Release, [3.0, 4.0], false)];
        assert_eq!(
            take(&mut selection, release),
            Some(([3.0, 4.0], How::Replace))
        );
        // Cancelled: its release is no click (none comes, but a stray one isn't taken).
        let cancelled = vec![
            event(Press, [3.0, 4.0], false),
            event(Cancel, [3.0, 4.0], false),
            event(Release, [3.0, 4.0], false),
        ];
        assert_eq!(take(&mut selection, cancelled), None);
    }

    /// A plain click at the view's centre, made at `at`, under the default filter, waiting
    /// for the meshes of `awaits`.
    fn waiting_click(at: Instant, awaits: Option<Of>) -> Click {
        Click {
            rect: square([50.0, 50.0], [100, 100], 3),
            how: How::Replace,
            after: None,
            frames: 0,
            filter: Filter::default(),
            at,
            awaits,
        }
    }

    #[test]
    fn a_click_with_no_model_built_is_let_go_at_once_and_said() {
        use LeftKind::{Press, Release};
        let mut selection = Selection::default();
        let mut view = View::default();
        let ctx = egui::Context::default();
        let frame = frame_of(vec![
            event(Press, [50.0, 50.0], false),
            event(Release, [50.0, 50.0], false),
        ]);
        selection.overlay(&mut view, &frame, &ctx);
        assert_eq!(selection.click, None);
        assert_eq!(selection.said(), Some(CLICK_NO_MODEL));
    }

    #[test]
    fn a_click_waiting_is_let_go_at_its_backstop_said_unless_its_models_rebuild_said_whats_gone() {
        // Waiting for the meshes of model 1, which no read has given the view (none in
        // flight in a default view, and none refused): kept to the backstop, then let go.
        let of = (1, Revision::from_number(1).unwrap());
        let mut selection = Selection::default();
        selection.published(Some((fused(1, 2), of.1)), of.0);
        let view = View::default();
        let at = Instant::now();
        selection.click = Some(waiting_click(at, Some(of)));
        selection.let_go_if_vain(&view, at + CLICK_WAIT - Duration::from_millis(1), false);
        assert!(selection.click.is_some());
        selection.let_go_if_vain(&view, at + CLICK_WAIT, false);
        assert_eq!(selection.click, None);
        assert_eq!(selection.said(), Some(CLICK_STALE));
        // What model 1's carry left gone is said, and stays over a click let go waiting for
        // its meshes.
        let gone = "The selected face is gone after the rebuild".to_owned();
        selection.said = Some(gone.clone());
        selection.gone_said = Some((of, gone.clone()));
        selection.click = Some(waiting_click(at, Some(of)));
        selection.let_go_if_vain(&view, at + CLICK_WAIT, false);
        assert_eq!(selection.click, None);
        assert_eq!(selection.said(), Some(gone.as_str()));
        // Said of another model, or since replaced by other words: the click's said over it.
        selection.gone_said = Some(((0, of.1), gone.clone()));
        selection.click = Some(waiting_click(at, Some(of)));
        selection.let_go_if_vain(&view, at + CLICK_WAIT, false);
        assert_eq!(selection.said(), Some(CLICK_STALE));
        selection.gone_said = Some((of, gone));
        selection.said = Some("Can't select it".to_owned());
        selection.click = Some(waiting_click(at, Some(of)));
        selection.let_go_if_vain(&view, at + CLICK_WAIT, false);
        assert_eq!(selection.said(), Some(CLICK_STALE));
    }

    #[test]
    fn a_mode_claiming_the_canvas_lets_a_waiting_click_and_the_press_and_hover_go() {
        let mut selection = Selection {
            click: Some(waiting_click(Instant::now(), None)),
            press: Some(Press {
                at_px: [1.0, 1.0],
                far: false,
            }),
            hover_asked: Some([1, 1]),
            hovered: Some(Hovered {
                body: 1,
                element: Element::Face(3),
                name: named("k7f2qx3m", face(3)),
                stable: true,
                found: None,
            }),
            ..Selection::default()
        };
        selection.canvas_claimed();
        assert_eq!(selection.click, None);
        assert_eq!(selection.press, None);
        assert_eq!(selection.hovered, None);
        assert_eq!(selection.hover_asked, None);
    }

    #[test]
    fn a_picks_square_is_the_band_about_the_pointers_pixel_clamped_into_the_view() {
        // 3 points at 1 and at 2 pixels a point: 3 and 6 pixels out on each side.
        assert_eq!((band_px(1.0), band_px(2.0), band_px(1.5)), (3, 6, 5));
        let rect = square([10.7, 20.2], [100, 50], 3);
        assert_eq!(
            rect,
            PixelRect {
                min: [7, 17],
                size: [7, 7]
            }
        );
        assert_eq!(centre_of(rect), [10, 20]);
        // Past the view's right and bottom, and above its top: the pixel clamped in.
        let rect = square([140.0, -9.0], [100, 50], 3);
        assert_eq!(centre_of(rect), [99, 0]);
        assert_eq!(rect.min, [96, -3]);
        assert!(rect.takes());
    }

    fn edge(index: u32) -> ElementRef {
        ElementRef {
            kind: ElementKind::Edge,
            index,
        }
    }

    #[test]
    fn names_a_rebuild_left_gone_are_dropped_the_rest_kept_and_counted_by_kind() {
        // Two faces and three edges, found; then two of the edges gone: dropped, and said
        // as a count of the edges selected before, the rest kept, in order.
        let names = [
            "k7f2qx3m.box.Face1",
            "k7f2qx3m.box.Edge1",
            "k7f2qx3m.box.Edge2",
            "k7f2qx3m.box.Face2",
            "k7f2qx3m.box.Edge3",
        ];
        let mut selection = Selection::default();
        selection.set(names.iter().map(|name| picked(name)).collect());
        let all = vec![
            found("k7f2qx3m", face(1), 0),
            found("k7f2qx3m", edge(1), 1),
            found("k7f2qx3m", edge(2), 2),
            found("k7f2qx3m", face(2), 3),
            found("k7f2qx3m", edge(3), 4),
        ];
        took(&mut selection, resolved(all, vec![], &[]));
        assert_eq!(selection.said(), None);
        took(
            &mut selection,
            resolved(
                vec![
                    found("k7f2qx3m", face(1), 0),
                    found("k7f2qx3m", edge(2), 2),
                    found("k7f2qx3m", face(2), 3),
                ],
                vec![gone(names[1], 1, None), gone(names[4], 4, None)],
                &[],
            ),
        );
        assert_eq!(selection.names(), [names[0], names[2], names[3]]);
        assert_eq!(
            selection.said(),
            Some("2 of 3 selected edges are gone after the rebuild")
        );
        // Then a face and the one edge left: each kind counted, faces first.
        took(
            &mut selection,
            resolved(
                vec![found("k7f2qx3m", face(1), 0)],
                vec![gone(names[2], 1, None), gone(names[3], 2, None)],
                &[],
            ),
        );
        assert_eq!(selection.names(), [names[0]]);
        assert_eq!(
            selection.said(),
            Some("1 of 2 selected faces and the selected edge are gone after the rebuild")
        );
        // The last: alone, it's said with why, in the core's words.
        took(
            &mut selection,
            resolved(vec![], vec![gone(names[0], 0, None)], &[]),
        );
        assert!(selection.is_empty());
        assert_eq!(
            selection.said(),
            Some("The selected face is gone after the rebuild: k7f2qx3m.box.Face1 names nothing")
        );
    }

    #[test]
    fn a_picking_answer_the_view_holds_frame_after_frame_is_taken_once() {
        // The view holds its last answer until a pick is asked again: here none is (the
        // pointer's pixel was asked already), so each frame's overlay sees frame 5's answer.
        // Taken once: what it found (nothing under the pointer) isn't found again over what's
        // hovered since.
        let of = (1, Revision::OPENED);
        let mut selection = Selection {
            of: Some(of),
            ..Selection::default()
        };
        let mut view = View::default();
        let rect = square([50.0, 50.0], [100, 100], 3);
        view.answered_as(
            of,
            Picking {
                frame: 5,
                rect,
                hit: None,
                edge: None,
            },
        );
        let mut frame = frame_of(Vec::new());
        frame.pointer_px = Some([50.0, 50.0]);
        selection.hover_asked = Some(centre_of(rect));
        let ctx = egui::Context::default();
        selection.overlay(&mut view, &frame, &ctx);
        assert_eq!(selection.taken, Some(5));
        assert_eq!(selection.hovered, None);
        let hovered = Hovered {
            body: 1,
            element: Element::Face(3),
            name: named("k7f2qx3m", face(3)),
            stable: true,
            found: None,
        };
        selection.hovered = Some(hovered.clone());
        selection.overlay(&mut view, &frame, &ctx);
        assert_eq!(view.picked().map(|picking| picking.frame), Some(5));
        assert_eq!(selection.hovered, Some(hovered));
    }

    // ---- Models built on the fake kernel, as the core's selection tests build them ----

    use silico_core::kernel::{
        BooleanOp, BuiltData, ElementCounts, ElementFate, History, InputHistory, Op, Topology,
    };

    fn kept(n: u32) -> ElementFate {
        ElementFate {
            kept: Some(n),
            ..ElementFate::default()
        }
    }

    /// A fuse's history in which its base, a 1 mm cube, is the result, its Face1 and Edge1
    /// reported by nothing, so both are named by their numbers, unstable (N3); the rest
    /// kept; the tool reported by nothing (the core's `numbering_fuse`).
    fn numbering_fuse() -> History {
        let none = |n: usize| vec![ElementFate::default(); n];
        let face1 = |rest: Vec<Vec<u32>>| -> Vec<Vec<u32>> {
            std::iter::once(vec![1, 2]).chain(rest).collect()
        };
        History {
            inputs: vec![
                InputHistory {
                    faces: none(1).into_iter().chain((2..=6).map(kept)).collect(),
                    edges: none(1).into_iter().chain((2..=12).map(kept)).collect(),
                    vertices: (1..=8).map(kept).collect(),
                },
                InputHistory {
                    faces: none(6),
                    edges: none(12),
                    vertices: none(8),
                },
            ],
            result: ElementCounts {
                faces: 6,
                edges: 12,
                vertices: 8,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: face1(vec![vec![]; 5]),
                outer_edges: face1(vec![vec![]; 5]),
                edge_vertices: vec![vec![]; 12],
                solid_faces: vec![(1..=6).collect()],
            },
        }
    }

    /// The numbering fuse after an edit of its tool: the base's Face1 kept as the result's
    /// Face2, its Face2 reported by nothing, so the result's Face1, named by its number, is
    /// another face (the core's `renumbering_fuse`).
    fn renumbering_fuse() -> History {
        let mut history = numbering_fuse();
        history.inputs[0].faces = vec![
            kept(2),
            ElementFate::default(),
            kept(3),
            kept(4),
            kept(5),
            kept(6),
        ];
        history
    }

    /// A fuse keeping its base whole, and the tool's Face1 as the result's Face1 (the
    /// core's `aliasing_fuse`): the result's Face1 has the tool's stable name beside the
    /// base's.
    fn aliasing_fuse() -> History {
        let mut tool_faces = vec![ElementFate::default(); 6];
        tool_faces[0] = kept(1);
        History {
            inputs: vec![
                InputHistory {
                    faces: (1..=6).map(kept).collect(),
                    edges: (1..=12).map(kept).collect(),
                    vertices: (1..=8).map(kept).collect(),
                },
                InputHistory {
                    faces: tool_faces,
                    edges: vec![ElementFate::default(); 12],
                    vertices: vec![ElementFate::default(); 8],
                },
            ],
            result: ElementCounts {
                faces: 6,
                edges: 12,
                vertices: 8,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![]; 6],
                outer_edges: vec![vec![]; 6],
                edge_vertices: vec![vec![]; 12],
                solid_faces: vec![(1..=6).collect()],
            },
        }
    }

    /// The aliasing fuse with the tool reported by nothing (the core's `plain_fuse`).
    fn plain_fuse() -> History {
        let mut history = aliasing_fuse();
        history.inputs[1].faces[0] = ElementFate::default();
        history
    }

    /// The history of a fuse whose tool is `tool` (the fake's description of it), as the
    /// core's carried-text proof gives them.
    fn fusing(tool: &str) -> History {
        if tool.contains("box(3,") {
            renumbering_fuse()
        } else if tool.contains("box(2,") {
            plain_fuse()
        } else if tool.contains("box(4,") {
            aliasing_fuse()
        } else {
            numbering_fuse()
        }
    }

    /// Two fuses: a 1 mm cube and a box `tool` long, fused, then fused with a cube `cube`
    /// long, rebuilt on the fake kernel and published, every feature's elements named.
    fn fused(tool: u32, cube: u32) -> Arc<Model> {
        let a_box = |id: &str, x: u32, length: u32| {
            format!(
                r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": {length}, "width": 1, "height": 1}}"#
            )
        };
        let fuse = |id: &str, base: &str, tool: &str| {
            format!(
                r#"{{"id": "{id}", "type": "fuse", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}}"#
            )
        };
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, tool),
            fuse("rrrrrrrr", "aaaaaaaa", "bbbbbbbb"),
            a_box("cccccccc", 9, cube),
            fuse("ssssssss", "rrrrrrrr", "cccccccc"),
        ];
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
            features.join(", ")
        );
        let checked = silico_core::document::Checked::new(
            silico_core::document::read(text.as_bytes()).unwrap(),
        )
        .unwrap();
        let engine =
            silico_core::recompute::Recompute::new(silico_core::fake::FakeKernel::new(2).unwrap())
                .with_sketches(silico_core::fake::FakeSketches::new());
        engine.kernel().answer_instead(|op| match op {
            Op::Boolean {
                op: BooleanOp::Fuse,
                base,
                tool,
            } => Some(silico_protocol::Reply::Built(BuiltData {
                shape: base.clone(),
                history: fusing(&silico_core::fake::FakeKernel::describe(tool)?),
            })),
            _ => None,
        });
        let Ok(Ok(silico_core::recompute::RunOutcome::Published(model))) = engine
            .start(Arc::new(checked))
            .wait_timeout(std::time::Duration::from_secs(120))
        else {
            panic!("not published")
        };
        for (id, _) in model.features() {
            model.element_map(id);
        }
        model
    }

    #[test]
    fn a_clicks_frames_asked_on_one_models_meshes_dont_count_against_its_wait_on_the_next() {
        use LeftKind::{Press, Release};
        // A click on model 1's meshes, asked in 30 frames and never answered; then model 2
        // published, and its meshes come: the click is asked in CLICK_FRAMES frames on them,
        // in full, and let go in the next.
        let model = fused(1, 2);
        let first = (1, Revision::from_number(1).unwrap());
        let second = (2, Revision::from_number(2).unwrap());
        let mut selection = Selection::default();
        selection.published(Some((Arc::clone(&model), first.1)), first.0);
        let mut view = View::default();
        let rect = square([50.0, 50.0], [100, 100], 3);
        let answer = Picking {
            frame: 5,
            rect,
            hit: None,
            edge: None,
        };
        view.answered_as(first, answer);
        let ctx = egui::Context::default();
        let click = frame_of(vec![
            event(Press, [50.0, 50.0], false),
            event(Release, [50.0, 50.0], false),
        ]);
        selection.overlay(&mut view, &click, &ctx);
        for _ in 1..30 {
            selection.overlay(&mut view, &frame_of(Vec::new()), &ctx);
        }
        assert_eq!(selection.click.map(|click| click.frames), Some(30));
        selection.published(Some((Arc::clone(&model), second.1)), second.0);
        // Waiting for model 2's meshes: no frame counted.
        for _ in 0..5 {
            selection.overlay(&mut view, &frame_of(Vec::new()), &ctx);
        }
        assert_eq!(
            selection.click.map(|click| click.awaits),
            Some(Some(second))
        );
        // Model 2's meshes come (the view's last answer, frame 5's, is before the click).
        view.answered_as(second, answer);
        for n in 1..=CLICK_FRAMES {
            selection.overlay(&mut view, &frame_of(Vec::new()), &ctx);
            assert_eq!(selection.click.map(|click| click.frames), Some(n));
        }
        selection.overlay(&mut view, &frame_of(Vec::new()), &ctx);
        assert_eq!(selection.click, None);
    }

    #[test]
    fn a_name_carried_into_a_model_stays_carried_through_a_pick_and_is_dropped_never_aliased() {
        // The core's proof (`carried_unstable_text_is_gone_though_the_element_it_finds_now_
        // has_a_stable_alias`), through the selection. Before: `rrrrrrrr.fuse.Face1`, fused
        // plainly with a 2 mm cube, names the second fuse's Face1, which has no other name:
        // picked, held unstable. Then the first fuse's tool made 3 long (the text names
        // another face) and the cube 4 (that face has a stable alias now,
        // `cccccccc.box.Face1`). Published before its names are, the name carried to it is
        // pending; a Ctrl-click picks another face meanwhile; once the names are, the
        // carried text is gone, said in a reference's words, never held by the alias a pick
        // of the text would hold.
        let name = "rrrrrrrr.fuse.Face1";
        let other = "aaaaaaaa.box.Face3";
        let revision = |n| Revision::from_number(n).unwrap();
        let after = fused(3, 4);
        let mut selection = Selection::default();
        selection.published(Some((fused(1, 2), revision(1))), 1);
        selection.pick(name, How::Replace);
        assert_eq!(states(&selection), [(name.to_owned(), "found")]);
        assert!(!selection.elements().next().unwrap().stable());
        selection.published(Some((Arc::new(after.unnamed()), revision(2))), 2);
        assert_eq!(states(&selection), [(name.to_owned(), "pending")]);
        selection.pick(other, How::Toggle);
        assert_eq!(
            states(&selection),
            [(name.to_owned(), "pending"), (other.to_owned(), "pending")]
        );
        // The same model, its names named (as the background names them).
        selection.published(Some((after, revision(2))), 2);
        assert_eq!(selection.names(), [other]);
        let said = selection.said().unwrap();
        assert!(said.starts_with(CARRIED_GONE), "{said}");
    }

    /// The words the carried `rrrrrrrr.fuse.Face1` is dropped with, gone after the rebuild
    /// at place 0: a reference's for an unstable face (references.rs's, N3, by hand).
    const CARRIED_GONE: &str = "The selected face is gone after the rebuild: selection.0, \
                                \"rrrrrrrr.fuse.Face1\", names a face, whose name is unstable \
                                (it came about as another element did, or is its number): an \
                                edit could make it another element";

    #[test]
    fn a_ctrl_click_of_a_carried_names_alias_is_held_as_the_carried_name_is_dropped() {
        // As the carried test, but the Ctrl-click is of the face's stable alias in the new
        // model, `cccccccc.box.Face1`: the element the carried text finds too. One read takes
        // each name its own way: the carried text gone, holding no place for the face, and
        // the pick held, never lost between two reads that each took the face once.
        let name = "rrrrrrrr.fuse.Face1";
        let alias = "cccccccc.box.Face1";
        let revision = |n| Revision::from_number(n).unwrap();
        let after = fused(3, 4);
        let mut selection = Selection::default();
        selection.published(Some((fused(1, 2), revision(1))), 1);
        selection.pick(name, How::Replace);
        selection.published(Some((Arc::new(after.unnamed()), revision(2))), 2);
        selection.pick(alias, How::Toggle);
        assert_eq!(
            states(&selection),
            [(name.to_owned(), "pending"), (alias.to_owned(), "pending")]
        );
        selection.published(Some((after, revision(2))), 2);
        assert_eq!(selection.names(), [alias]);
        assert_eq!(states(&selection), [(alias.to_owned(), "found")]);
        assert!(selection.elements().next().unwrap().stable());
        let said = selection.said().unwrap();
        assert!(said.starts_with(CARRIED_GONE), "{said}");
    }

    #[test]
    fn a_pick_of_the_same_text_as_a_carried_name_is_held_as_the_carried_name_is_dropped() {
        // The carried text and a pick of the same text in one selection (the carried one's
        // last answer an older model's face, so a Ctrl-click of the text found in this one
        // is added beside it): carried, the text is gone; picked on this model, it's held by
        // the face's stable alias.
        let name = "rrrrrrrr.fuse.Face1";
        let alias = "cccccccc.box.Face1";
        let after = fused(3, 4);
        let mut selection = Selection::default();
        selection.published(
            Some((Arc::clone(&after), Revision::from_number(2).unwrap())),
            2,
        );
        let item = |carry, last| {
            Item::Element(Picked {
                name: name.to_owned(),
                state: State::Waiting { why: None, last },
                carry,
                found_in: None,
            })
        };
        let older = found("ssssssss", face(2), 0);
        selection.set(vec![
            item(Carry::Refresh, Some(older)),
            item(Carry::Pick, None),
        ]);
        selection.read(&after);
        assert_eq!(selection.names(), [alias]);
        assert_eq!(states(&selection), [(alias.to_owned(), "found")]);
        let said = selection.said().unwrap();
        assert!(said.starts_with(CARRIED_GONE), "{said}");
    }

    #[test]
    fn a_ctrl_click_of_a_selected_elements_other_stable_name_removes_it() {
        // Fused plainly with a 2 mm tool, the first fuse keeps its base whole, so the second
        // fuse's base Face1 is `aaaaaaaa.box.Face1`; that fuse aliases its 4 mm cube's Face1
        // onto it (the core's `aliasing_fuse`), so the face has a second stable name,
        // `cccccccc.box.Face1`. Each pick of a stable name is held as picked: two names, one
        // element. A Ctrl-click of the second is of the element selected, and removes it.
        let model = fused(2, 4);
        let mut selection = Selection::default();
        selection.published(Some((model, Revision::from_number(1).unwrap())), 1);
        selection.pick("aaaaaaaa.box.Face1", How::Replace);
        assert_eq!(
            states(&selection),
            [("aaaaaaaa.box.Face1".to_owned(), "found")]
        );
        let first = selection
            .elements()
            .next()
            .unwrap()
            .found()
            .unwrap()
            .clone();
        assert!(first.stable());
        assert_eq!(
            (first.feature.as_str(), first.element),
            ("ssssssss", face(1))
        );
        selection.pick("cccccccc.box.Face1", How::Toggle);
        assert!(selection.is_empty(), "{:?}", selection.names());
        assert_eq!(selection.said(), None);
        // Picked again by it, it's held by the name picked this time.
        selection.pick("cccccccc.box.Face1", How::Toggle);
        assert_eq!(selection.names(), ["cccccccc.box.Face1"]);
    }

    #[test]
    fn the_outlines_are_the_prototypes_selected_edge_blue_and_hovered_edge_white() {
        // DESIGN.md's *Viewport*: "hover: … 3 pt white edges; selection: … 3.5 pt `#5b95ff`
        // edges".
        assert_eq!(
            outlines(),
            Outlines {
                selected: [0x5b, 0x95, 0xff],
                hovered: [0xff, 0xff, 0xff],
            }
        );
    }

    #[test]
    fn the_click_distance_is_eguis_in_points_taken_to_pixels_at_two_pixels_a_point() {
        // egui's click distance is 6 points: 12 pixels at 2 pixels a point. With no model
        // built, a click is let go at once and said (CLICK_NO_MODEL), so what's said tells
        // whether the press and release were taken for a click.
        use LeftKind::{Press, Release};
        let ctx = egui::Context::default();
        assert_eq!(
            ctx.options(|options| options.input_options.max_click_dist),
            6.0
        );
        let clicked = |apart: f32| {
            let mut selection = Selection::default();
            let mut frame = frame_of(vec![
                event(Press, [50.0, 50.0], false),
                event(Release, [50.0 + apart, 50.0], false),
            ]);
            frame.pixels_per_point = 2.0;
            selection.overlay(&mut View::default(), &frame, &ctx);
            selection.said() == Some(CLICK_NO_MODEL)
        };
        assert!(clicked(0.0));
        assert!(clicked(12.0));
        assert!(!clicked(12.5));
    }

    fn mesh_element(name: Arc<Name>) -> MeshElement {
        MeshElement {
            name,
            stable: true,
            count: None,
        }
    }

    /// Feature `feature`'s body, its faces named `faces` in its mesh's order (no edges): what
    /// highlights and picks read of it.
    fn view_body(feature: &str, faces: &[Arc<Name>]) -> ViewBody {
        ViewBody {
            feature: fid(feature),
            key: Key::parse(&"01".repeat(32)).unwrap(),
            level: 0.1,
            deflection: 0.1,
            bounds: None,
            serves: [0.1, 0.2],
            held: false,
            mesh: Arc::new(silico_protocol::Mesh {
                positions: Vec::new(),
                normals: Vec::new(),
                triangles: Vec::new(),
                face_triangles: Vec::new(),
                valid: None,
                edge_points: Vec::new(),
                edge_point_counts: Vec::new(),
            }),
            faces: faces.iter().cloned().map(mesh_element).collect(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn a_hover_moving_onto_another_element_is_of_that_element() {
        // Body 1's two faces; the pointer over Face1, then over Face2: the hover is Face2's.
        let bodies = BTreeMap::from([(
            1,
            view_body(
                "k7f2qx3m",
                &[named("k7f2qx3m", face(1)), named("k7f2qx3m", face(2))],
            ),
        )]);
        let rect = square([50.0, 50.0], [100, 100], 3);
        let over = |n: u32| Picking {
            frame: 1,
            rect,
            hit: Some(crate::renderer::Pick {
                body: 1,
                hit: Element::Face(n),
                pixel: [50, 50],
                point: None,
            }),
            edge: None,
        };
        let hovered = |selection: &Selection| {
            selection
                .hovered()
                .map(|hovered| (hovered.body, hovered.element, hovered.name.to_string()))
        };
        let mut selection = Selection::default();
        selection.hover_answered(&over(1), &bodies);
        assert_eq!(
            hovered(&selection),
            Some((1, Element::Face(1), "k7f2qx3m.box.Face1".to_owned()))
        );
        selection.hover_answered(&over(2), &bodies);
        assert_eq!(
            hovered(&selection),
            Some((1, Element::Face(2), "k7f2qx3m.box.Face2".to_owned()))
        );
    }

    #[test]
    fn a_feature_lights_its_bodys_every_face_or_else_the_faces_named_as_its_own() {
        // Body 1, the fuse rrrrrrrr: its Face1 named as aaaaaaaa's, its Face2 as its own,
        // its Face3 as bbbbbbbb's. Body 2, the box cccccccc: both faces its own.
        use Element::Face;
        let bodies = BTreeMap::from([
            (
                1,
                view_body(
                    "rrrrrrrr",
                    &[
                        named("aaaaaaaa", face(1)),
                        named("rrrrrrrr", face(2)),
                        named("bbbbbbbb", face(3)),
                    ],
                ),
            ),
            (
                2,
                view_body(
                    "cccccccc",
                    &[named("cccccccc", face(1)), named("cccccccc", face(2))],
                ),
            ),
        ]);
        let lit = |id: &str| highlights(&[Item::Feature(fid(id))], &bodies, None);
        let set = |lit: &[(u32, Element)]| lit.iter().copied().collect::<BTreeSet<_>>();
        // A body: its whole, and no other body's faces.
        assert_eq!(
            lit("rrrrrrrr"),
            set(&[(1, Face(1)), (1, Face(2)), (1, Face(3))])
        );
        assert_eq!(lit("cccccccc"), set(&[(2, Face(1)), (2, Face(2))]));
        // A feature consumed: the faces named as its own, in the body they're in.
        assert_eq!(lit("aaaaaaaa"), set(&[(1, Face(1))]));
        assert_eq!(lit("bbbbbbbb"), set(&[(1, Face(3))]));
        assert_eq!(lit("zzzzzzzz"), set(&[]));
        // An element is drawn only if the answer it's drawn by is of the bodies' model.
        let of = (3, Revision::from_number(3).unwrap());
        let element = |found_in| {
            Item::Element(Picked {
                name: "cccccccc.box.Face2".to_owned(),
                state: State::Found(found("cccccccc", face(2), 0)),
                carry: Carry::Pick,
                found_in,
            })
        };
        assert_eq!(
            highlights(&[element(Some(of))], &bodies, Some(of)),
            set(&[(2, Face(2))])
        );
        let older = (2, Revision::from_number(2).unwrap());
        assert_eq!(
            highlights(&[element(Some(older))], &bodies, Some(of)),
            set(&[])
        );
        assert_eq!(highlights(&[element(Some(of))], &bodies, None), set(&[]));
    }

    // ---- Picks together (M5.7's box) ----

    /// One 2 by 1 by 1 box, rebuilt on the fake kernel and published at revision 1, its
    /// elements named: `aaaaaaaa.box.Edge1` to `Edge12`, `Face1` to `Face6`.
    fn one_box() -> Selection {
        let mut selection = Selection::default();
        selection.published(Some((box_model(), Revision::from_number(1).unwrap())), 1);
        selection
    }

    /// [`one_box`]'s model.
    fn box_model() -> Arc<Model> {
        let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 2, "width": 1, "height": 1}]}"#;
        let checked = silico_core::document::Checked::new(
            silico_core::document::read(text.as_bytes()).unwrap(),
        )
        .unwrap();
        let engine =
            silico_core::recompute::Recompute::new(silico_core::fake::FakeKernel::new(1).unwrap())
                .with_sketches(silico_core::fake::FakeSketches::new());
        let Ok(Ok(silico_core::recompute::RunOutcome::Published(model))) = engine
            .start(Arc::new(checked))
            .wait_timeout(std::time::Duration::from_secs(120))
        else {
            panic!("not published")
        };
        for (id, _) in model.features() {
            model.element_map(id);
        }
        model
    }

    fn edges(numbers: &[u32]) -> Vec<String> {
        numbers
            .iter()
            .map(|n| format!("aaaaaaaa.box.Edge{n}"))
            .collect()
    }

    #[test]
    fn picks_together_are_one_change_one_revision_and_an_empty_replace_clears() {
        let mut selection = one_box();
        let before = selection.version;
        selection.pick_all(&edges(&[1, 2, 3]), How::Replace);
        assert_eq!(selection.names(), edges(&[1, 2, 3]));
        assert_eq!(
            selection.version,
            before + 1,
            "one revision for three names"
        );
        assert_eq!(selection.said(), None);
        // The same again: nothing changes, no revision.
        selection.pick_all(&edges(&[1, 2, 3]), How::Replace);
        assert_eq!(selection.version, before + 1);
        // An empty replace clears, as Esc does: once.
        selection.pick_all(&[], How::Replace);
        assert!(selection.is_empty());
        assert_eq!(selection.version, before + 2);
        selection.pick_all(&[], How::Replace);
        assert_eq!(selection.version, before + 2);
        // An empty add or toggle changes nothing.
        selection.pick_all(&edges(&[4]), How::Replace);
        let version = selection.version;
        selection.pick_all(&[], How::Add);
        selection.pick_all(&[], How::Toggle);
        assert_eq!(selection.names(), edges(&[4]));
        assert_eq!(selection.version, version);
    }

    #[test]
    fn picks_together_add_with_shift_and_toggle_with_ctrl_each_element_once() {
        let mut selection = one_box();
        selection.pick_all(&edges(&[1, 2]), How::Replace);
        // Add: what's selected stays where it was, the rest after it, in the order named.
        selection.pick_all(&edges(&[3, 2, 4]), How::Add);
        assert_eq!(selection.names(), edges(&[1, 2, 3, 4]));
        // Toggle: those selected removed, the others added.
        let version = selection.version;
        selection.pick_all(&edges(&[2, 5, 4]), How::Toggle);
        assert_eq!(selection.names(), edges(&[1, 3, 5]));
        assert_eq!(selection.version, version + 1);
        // Named twice, toggled once: Edge6 added, Edge1 removed.
        selection.pick_all(&edges(&[6, 6, 1, 1]), How::Toggle);
        assert_eq!(selection.names(), edges(&[3, 5, 6]));
        // A replace named twice holds it once.
        selection.pick_all(&edges(&[7, 7]), How::Replace);
        assert_eq!(selection.names(), edges(&[7]));
        // A feature selected stays through an add, and goes with a replace.
        let id = FeatureId::new("aaaaaaaa").unwrap();
        selection.select_feature(id.clone());
        selection.pick_all(&edges(&[8]), How::Add);
        assert_eq!(selection.items()[0], Item::Feature(id));
        assert_eq!(selection.names(), edges(&[8]));
        selection.pick_all(&edges(&[9]), How::Replace);
        assert_eq!(selection.names(), edges(&[9]));
        assert_eq!(selection.items().len(), 1);
    }

    #[test]
    fn a_name_not_found_among_picks_together_is_left_out_and_said_and_too_many_are_refused() {
        let mut selection = one_box();
        let mut names = edges(&[1]);
        names.push("aaaaaaaa.box.Edge99".to_owned());
        names.extend(edges(&[2]));
        selection.pick_all(&names, How::Replace);
        assert_eq!(selection.names(), edges(&[1, 2]));
        let said = selection.said().unwrap();
        assert!(
            said.starts_with("Can't select aaaaaaaa.box.Edge99: "),
            "{said}"
        );
        // Two not found: counted.
        names.push("aaaaaaaa.box.Face77".to_owned());
        selection.pick_all(&names, How::Replace);
        let said = selection.said().unwrap();
        assert!(
            said.starts_with("Can't select 2 of the 4 elements: "),
            "{said}"
        );
        // More than the core reads at once: refused whole, the selection kept, and said.
        let version = selection.version;
        let many: Vec<String> = (0..=silico_core::selection::MAX_SELECTED)
            .map(|n| format!("aaaaaaaa.box.Edge{}", n % 12 + 1))
            .collect();
        selection.pick_all(&many, How::Replace);
        assert_eq!(selection.names(), edges(&[1, 2]));
        assert_eq!(selection.version, version);
        assert_eq!(
            selection.said(),
            Some(
                "Can't select 10001 elements: a selection holds at most 10000; select fewer, \
                 or use a filter"
            )
        );
        // With no model built: said, nothing changed.
        let mut unbuilt = Selection::default();
        unbuilt.pick_all(&edges(&[1, 2]), How::Replace);
        assert!(unbuilt.is_empty());
        assert_eq!(
            unbuilt.said(),
            Some("Can't select 2 elements: no model is built yet")
        );
    }

    #[test]
    fn a_click_adds_with_add_and_keeps_what_is_selected() {
        let mut selection = one_box();
        selection.pick("aaaaaaaa.box.Edge1", How::Replace);
        selection.pick("aaaaaaaa.box.Edge2", How::Add);
        selection.pick("aaaaaaaa.box.Edge1", How::Add);
        assert_eq!(selection.names(), edges(&[1, 2]));
    }

    #[test]
    fn picks_together_keep_a_carried_name_carried_and_its_alias_picked() {
        // As `a_ctrl_click_of_a_carried_names_alias_is_held_as_the_carried_name_is_dropped`,
        // the Ctrl-click a box's toggle: the carried text stays carried through the batch,
        // so once the names are named it's gone, said, and the alias held.
        let name = "rrrrrrrr.fuse.Face1";
        let alias = "cccccccc.box.Face1";
        let revision = |n| Revision::from_number(n).unwrap();
        let after = fused(3, 4);
        let mut selection = Selection::default();
        selection.published(Some((fused(1, 2), revision(1))), 1);
        selection.pick(name, How::Replace);
        selection.published(Some((Arc::new(after.unnamed()), revision(2))), 2);
        selection.pick_all(&[alias.to_owned()], How::Toggle);
        assert_eq!(
            states(&selection),
            [(name.to_owned(), "pending"), (alias.to_owned(), "pending")]
        );
        selection.published(Some((after, revision(2))), 2);
        assert_eq!(selection.names(), [alias]);
        let said = selection.said().unwrap();
        assert!(said.starts_with(CARRIED_GONE), "{said}");
    }

    #[test]
    fn picks_together_on_a_model_not_named_yet_wait_and_are_found_once_it_is() {
        let model = box_model();
        let revision = Revision::from_number(1).unwrap();
        let mut selection = Selection::default();
        selection.published(Some((Arc::new(model.unnamed()), revision)), 1);
        let version = selection.version;
        selection.pick_all(&edges(&[1, 2]), How::Replace);
        assert_eq!(
            states(&selection),
            [
                ("aaaaaaaa.box.Edge1".to_owned(), "pending"),
                ("aaaaaaaa.box.Edge2".to_owned(), "pending")
            ]
        );
        assert_eq!(selection.version, version + 1);
        assert!(selection.again);
        // The same model, named: asked again, and found.
        selection.published(Some((model, revision)), 1);
        assert_eq!(
            states(&selection),
            [
                ("aaaaaaaa.box.Edge1".to_owned(), "found"),
                ("aaaaaaaa.box.Edge2".to_owned(), "found")
            ]
        );
    }

    #[test]
    fn picks_together_that_would_overfill_the_selection_with_whats_there_are_refused() {
        // 9,995 names selected already (waiting), and six faces added: 10,001, one past what
        // the core reads at once. Refused whole, said, nothing changed.
        let mut selection = one_box();
        let held: Vec<Item> = (1..=9_995)
            .map(|n| picked(&format!("zzzzzzzz.box.Edge{n}")))
            .collect();
        selection.set(held.clone());
        let version = selection.version;
        let faces: Vec<String> = (1..=6).map(|n| format!("aaaaaaaa.box.Face{n}")).collect();
        selection.pick_all(&faces, How::Add);
        assert_eq!(selection.items(), held.as_slice());
        assert_eq!(selection.version, version);
        assert_eq!(
            selection.said(),
            Some(
                "Can't select 10001 elements: a selection holds at most 10000; select fewer, \
                 or use a filter"
            )
        );
        // Five fit: taken (the read that follows drops the 9,995, which no model holds).
        selection.pick_all(&faces[..5], How::Add);
        assert_eq!(selection.names(), faces[..5]);
    }

    #[test]
    fn picks_together_scroll_the_browser_to_the_first_added_and_a_removal_scrolls_nowhere() {
        let mut selection = one_box();
        selection.pick_all(&edges(&[1, 2]), How::Replace);
        assert_eq!(
            selection.scroll_target().map(FeatureId::as_str),
            Some("aaaaaaaa")
        );
        selection.scrolled();
        selection.pick_all(&edges(&[2]), How::Toggle);
        assert_eq!(selection.names(), edges(&[1]));
        assert_eq!(selection.scroll_target(), None);
    }

    #[test]
    fn the_hovers_answer_is_heard_for_the_frame_it_was_taken_and_a_clicks_never() {
        use LeftKind::{Press, Release};
        let model = fused(1, 2);
        let of = (1, Revision::from_number(1).unwrap());
        let mut selection = Selection::default();
        selection.published(Some((model, of.1)), of.0);
        let mut view = View::default();
        let hover = square([50.0, 50.0], [100, 100], 3);
        view.answered_as(
            of,
            Picking {
                frame: 5,
                rect: hover,
                hit: None,
                edge: None,
            },
        );
        let ctx = egui::Context::default();
        selection.overlay(&mut view, &frame_of(Vec::new()), &ctx);
        assert_eq!(selection.heard(), Some(&([50, 50], None)));
        // Heard in that frame alone.
        selection.overlay(&mut view, &frame_of(Vec::new()), &ctx);
        assert_eq!(selection.heard(), None);
        // A click at (20, 20), and its answer: the click's, never heard as the hover's.
        let click = frame_of(vec![
            event(Press, [20.0, 20.0], false),
            event(Release, [20.0, 20.0], false),
        ]);
        selection.overlay(&mut view, &click, &ctx);
        assert!(selection.click.is_some());
        view.answered_as(
            of,
            Picking {
                frame: 6,
                rect: square([20.0, 20.0], [100, 100], 3),
                hit: None,
                edge: None,
            },
        );
        selection.overlay(&mut view, &frame_of(Vec::new()), &ctx);
        assert!(selection.click.is_none(), "the click's answer not taken");
        assert_eq!(selection.heard(), None);
    }
}
