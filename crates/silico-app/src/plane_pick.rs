//! Create Sketch's pick of a plane or a flat face (docs/design/m5.0-app.md, section 9's
//! *Sketch mode*; the approved prototype's `startPickPlane`, `endPickPlane`, `sketchOn`, and
//! its viewport hooks in js/interact.js, and `VP.pickPlane` in js/viewport.js; Fusion's
//! Create Sketch, docs/ux/fusion.md: "pick an origin plane, a work plane, or a planar face").
//!
//! Create Sketch (the toolbar's button, CREATE's item, the canvas's menu's item, and the empty
//! part's hint's button) begins it, the selection cleared, as the prototype's
//! `startPickPlane` clears it. While it's on, the viewport draws all three origin planes
//! (`crate::planes`), the canvas belongs to it (no selection, hover, box, or Select Other),
//! a hint at the canvas's top says "Pick an origin plane or a flat face to sketch on" with
//! Cancel (`crate::canvas_hint`), the toolbar's Create Sketch is drawn on, and the status bar
//! says what to pick. What the pointer is over ([`Under`]) is cast on the CPU, as Select
//! Other's right click is (`crate::select_other`), on the frame's own camera: the origin
//! planes the frame drew ([`crate::planes::under`]) and the face the pointer's ray meets
//! first (`SelectOther::face_met`), whichever the ray meets first, a
//! face before a plane it lies on (within [`TIE_MM`], the prototype's `+ 0.01`). A plane
//! under the pointer is drawn lit; a face, once `elements` says it's a plane (read as the
//! canvas's menu reads one, `crate::measures`), is drawn hovered.
//!
//! A click (a left press and its release within egui's click distance) on a plane starts a
//! sketch on it (`Tab::create_sketch`), and on a flat face one on the face by the name a
//! store writes for it (`Tab::create_sketch_on_face`, as Sketch on this face does), the
//! pick ended. A click on a face whose type isn't read yet waits for its read, and then does
//! the same. A click on a face that isn't flat, or one no store can name, is refused, said
//! in a toast in the prototype's words' style, and the pick goes on; a click on nothing
//! does nothing, as the prototype's. Cancel, Esc, a dialog or a sketch opened, or the tab
//! closing ends it.
//!
//! Where the prototype is silent, the app follows Fusion and SolidWorks: the nearest thing
//! under the pointer is what's picked, so a face that isn't flat in front of a plane hides
//! it (the prototype's `VP.pickPlane` takes the plane behind it); and a plane's row in the
//! browser picks it too (`crate::panes`), as Fusion's browser and SolidWorks'
//! FeatureManager tree give a plane while a sketch's plane is asked for, which also gives
//! the keyboard a way to pick one.

use std::collections::BTreeMap;
use std::sync::Arc;

use silico_core::command::{Revision, SessionId, Sessions};
use silico_core::document::OriginPlane;
use silico_core::kernel::Clients;
use silico_core::naming::Name;
use silico_protocol::vec3;
use silico_render::render::ray;

use crate::measures::{Measured, Measures, Of};
use crate::renderer::Element;
use crate::select_other::{Met, SelectOther};
use crate::selection::{Selection, centre_of, distance, square};
use crate::viewport::{LeftKind, View, ViewFrame};

/// How much nearer along the pointer's ray, in mm, a plane must be met than a face to be
/// picked before it: the prototype's `fhAny.distance <= ph.distance + 0.01`, so a face lying
/// on an origin plane (a pad's foot on XY) is picked, not the plane its triangles round about.
pub const TIE_MM: f64 = 0.01;

/// What the pointer is over in the pick: an origin plane, or a face of a body.
#[derive(Debug, Clone, PartialEq)]
pub enum Under {
    Plane(OriginPlane),
    Face(Face),
}

/// A face under the pointer: its body's number, its element, and its name as the body's
/// mesh names it.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub body: u32,
    pub element: Element,
    pub name: Arc<Name>,
}

impl Face {
    /// Its label, as the canvas names it ("Face6").
    fn label(&self) -> String {
        match self.element {
            Element::Face(n) => format!("Face{n}"),
            Element::Edge(n) => format!("Edge{n}"),
            Element::Vertex(n) => format!("Vertex{n}"),
        }
    }
}

/// What a face's read said of it: whether it's a plane (or why it couldn't be read), and the
/// name a store writes for it (or why none can be).
#[derive(Debug, Clone, PartialEq)]
struct Typed {
    planar: Result<bool, String>,
    stored: Result<Arc<Name>, String>,
}

/// What the pick asks of the tab.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// A sketch on this origin plane, the pick ended.
    Plane(OriginPlane),
    /// A sketch on the face a store names so, the pick ended.
    Face(Arc<Name>),
    /// A click refused: said, the pick going on.
    Say(String),
}

/// The pick: what the pointer is over, a press on the canvas waiting for its release, a click
/// on a face waiting for the face's type, and each face read for its type on the model its
/// reads are of, with the read out.
#[derive(Default)]
pub struct PlanePick {
    under: Option<Under>,
    press: Option<[f32; 2]>,
    clicked: Option<Face>,
    typed: BTreeMap<String, Typed>,
    model: Option<(u64, Revision)>,
    read: Measures,
    reading: Option<Reading>,
    asked: u64,
}

/// A face's read out: its name as the mesh names it, what the read is of, and the name a
/// store writes for it (or why none can be), as the selection's model found it.
#[derive(Debug, Clone)]
struct Reading {
    name: String,
    of: Of,
    stored: Result<Arc<Name>, String>,
}

/// What a frame gives the pick: the view and its frame, Select Other's boxes to cast on, the
/// selection (whose model names a face), whether the view's bodies are of that model, and
/// egui's click distance, in points.
pub(crate) struct Inputs<'a> {
    pub view: &'a View,
    pub frame: &'a ViewFrame,
    pub select_other: &'a SelectOther,
    pub selection: &'a Selection,
    pub current: bool,
    pub click_distance: f32,
}

impl PlanePick {
    /// The origin plane under the pointer, if one is: drawn lit.
    pub fn lit_plane(&self) -> Option<OriginPlane> {
        match self.under {
            Some(Under::Plane(plane)) => Some(plane),
            _ => None,
        }
    }

    /// The face under the pointer, once its read says it's a plane: drawn hovered.
    pub fn lit_face(&self) -> Option<&Face> {
        match &self.under {
            Some(Under::Face(face)) if self.planar(face) == Some(true) => Some(face),
            _ => None,
        }
    }

    /// What the pointer is over.
    pub fn under(&self) -> Option<&Under> {
        self.under.as_ref()
    }

    /// Whether a click on a face waits for the face's type.
    pub fn waiting(&self) -> bool {
        self.clicked.is_some()
    }

    /// Whether `face` is a plane, as its read said; none until it has.
    fn planar(&self, face: &Face) -> Option<bool> {
        self.typed
            .get(face.name.to_string().as_str())
            .map(|typed| typed.planar.as_ref().is_ok_and(|planar| *planar))
    }

    /// The canvas's frame given to the pick: its left button's clicks taken, what the pointer
    /// is over found, and each face read for its type as it's needed; what the pick asks of
    /// the tab, in order, and whether what it lights changed (for the next frame to draw).
    pub(crate) fn frame<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        inputs: &Inputs<'_>,
    ) -> (Vec<Act>, bool) {
        let _span = tracing::debug_span!("plane_pick_frame").entered();
        let model = inputs.selection.model_of();
        if model != self.model {
            // Another model: what was read of its faces is of the old one.
            self.model = model;
            self.typed.clear();
            self.clicked = None;
            self.reading = None;
        }
        let before = (self.lit_plane(), self.lit_face().cloned());
        let click_px = inputs.click_distance * inputs.frame.pixels_per_point;
        let mut acts = Vec::new();
        for event in &inputs.frame.left {
            match event.kind {
                LeftKind::Press => self.press = Some(event.at_px),
                LeftKind::Release => {
                    let pressed = self.press.take();
                    if pressed.is_some_and(|at| distance(at, event.at_px) <= click_px)
                        && let Some(under) = under_at(inputs, event.at_px)
                    {
                        acts.extend(self.click(under));
                    }
                }
                LeftKind::Cancel => self.press = None,
                LeftKind::Move | LeftKind::DoubleClick => {}
            }
        }
        self.under = inputs
            .frame
            .pointer_px
            .and_then(|at_px| under_at(inputs, at_px));
        self.read_type(sessions, session, inputs.selection);
        if let Some(face) = self.clicked.clone()
            && let Some(typed) = self.typed.get(face.name.to_string().as_str()).cloned()
        {
            self.clicked = None;
            acts.extend(decided(&face, &typed));
        }
        let after = (self.lit_plane(), self.lit_face().cloned());
        (acts, before != after)
    }

    /// A click on `under`: a plane's sketch, or a face's once its type is known (waiting for
    /// it until then).
    fn click(&mut self, under: Under) -> Option<Act> {
        match under {
            Under::Plane(plane) => Some(Act::Plane(plane)),
            Under::Face(face) => match self.typed.get(face.name.to_string().as_str()) {
                Some(typed) => decided(&face, typed),
                None => {
                    self.clicked = Some(face);
                    None
                }
            },
        }
    }

    /// The type read of the face a click waits on, or else of the face under the pointer if
    /// it isn't known yet; polled, and its answer kept once it comes.
    fn read_type<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        selection: &Selection,
    ) {
        let hovered = match &self.under {
            Some(Under::Face(face)) => Some(face),
            _ => None,
        };
        let wanted = self
            .clicked
            .as_ref()
            .or(hovered)
            .filter(|face| !self.typed.contains_key(face.name.to_string().as_str()))
            .cloned();
        let Some(model) = self.model else {
            self.read.ask(sessions, session, None, None);
            return;
        };
        if let Some(face) = &wanted
            && self
                .reading
                .as_ref()
                .is_none_or(|reading| reading.name != face.name.to_string())
        {
            match selection.resolve_one(face.name.to_string().as_str()) {
                Some(found) => {
                    self.asked = self.asked.wrapping_add(1);
                    let of = Of {
                        version: self.asked,
                        model,
                    };
                    self.reading = Some(Reading {
                        name: face.name.to_string(),
                        of,
                        stored: found.stored.clone().map_err(|why| why.message().to_owned()),
                    });
                    self.read.ask(
                        sessions,
                        session,
                        Some(of),
                        Some(Measured::One {
                            body: found.feature.clone(),
                            name: Arc::clone(&found.name),
                        }),
                    );
                }
                None => {
                    // Never, for a face of the model the selection is read against: said all
                    // the same, as a store's refusal of it would be.
                    let why = format!("{} isn't in the model as it's built now", face.label());
                    self.typed.insert(
                        face.name.to_string(),
                        Typed {
                            planar: Err(why.clone()),
                            stored: Err(why),
                        },
                    );
                }
            }
        }
        self.read.poll(sessions, session);
        let Some(reading) = self.reading.clone() else {
            return;
        };
        let answered = self
            .read
            .answers(reading.of)
            .and_then(|answers| answers.element.as_ref())
            .map(|element| match element {
                // `elements` gives a planar face alone its normal.
                Ok(entry) => Ok(entry.normal.is_some()),
                Err(problems) => Err(problems.errors().first().map_or_else(
                    || "Its face couldn't be read".to_owned(),
                    |error| error.message().to_owned(),
                )),
            });
        if let Some(planar) = answered {
            tracing::debug!(name = %reading.name, ?planar, "plane_pick_face_typed");
            self.typed.insert(
                reading.name,
                Typed {
                    planar,
                    stored: reading.stored,
                },
            );
            self.reading = None;
            self.read.ask(sessions, session, None, None);
        }
    }
}

/// What a click on `face` asks, its type known: a sketch on it if it's a plane a store can
/// name, or else why not, said.
fn decided(face: &Face, typed: &Typed) -> Option<Act> {
    let label = face.label();
    Some(match (&typed.planar, &typed.stored) {
        (Ok(true), Ok(stored)) => Act::Face(Arc::clone(stored)),
        (Ok(true), Err(why)) => Act::Say(format!("{label} can't be sketched on: {why}")),
        (Ok(false), _) => Act::Say(format!(
            "{label} isn't flat. Pick an origin plane or a flat face to sketch on."
        )),
        (Err(why), _) => Act::Say(format!("{label} couldn't be read: {why}")),
    })
}

/// What's under the pixel at `at_px` (physical pixels from the canvas's top left) of
/// `inputs`' frame: the origin plane or the face its ray meets first, a face before a plane
/// within [`TIE_MM`] of it; none where neither is, or while a body drawn has its boxes still
/// to come, or where a face of bodies not of the selection's model (not named yet) is met
/// first.
fn under_at(inputs: &Inputs<'_>, at_px: [f32; 2]) -> Option<Under> {
    let frame = inputs.frame;
    let pixel = centre_of(square(at_px, frame.size_px, 0)).map(|c| f64::from(c) + 0.5);
    let plane = crate::planes::under(
        &frame.camera,
        frame.size_px,
        pixel,
        inputs.view.planes_drawn(),
    );
    let face = match inputs.select_other.face_met(inputs.view, frame, pixel) {
        Met::Face(hit) => Some(hit),
        Met::Empty => None,
        // Not known yet: nothing, till the boxes are made (their thread asks for a frame).
        Met::Waiting => return None,
    };
    let Some(hit) = face else {
        return plane.map(|(plane, _)| Under::Plane(plane));
    };
    let (from, along) = ray(&frame.camera, frame.size_px, pixel)?;
    let met = vec3::dot(vec3::sub(hit.point, from), along);
    if let Some((plane, at)) = plane
        && at + TIE_MM < met
    {
        return Some(Under::Plane(plane));
    }
    if !inputs.current {
        return None;
    }
    let named = inputs.view.bodies().get(&hit.body)?.name_of(hit.element)?;
    Some(Under::Face(Face {
        body: hit.body,
        element: hit.element,
        name: Arc::clone(&named.name),
    }))
}
