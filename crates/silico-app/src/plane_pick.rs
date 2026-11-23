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
//! the same, unless another model is published first (let go, said in
//! [`PICK_CLICK_LET_GO`]'s words). A click is never held for anything else: one let go in
//! the frame the view takes a new table of bodies (made on the table the frame before drew),
//! one made while a body drawn has its boxes still to come (made off the UI thread as the
//! view takes a mesh, tens of milliseconds), or one on a face of an older model's meshes
//! than the one published, makes nothing and says so ([`PICK_NOT_READY`]: click again); the
//! pointer goes on lighting what it meets among what's known meanwhile. An origin plane met
//! first is taken whatever meshes the view draws, while the published model's are read or
//! their read is refused: it's drawn, and named, by no model. A newer click supersedes one
//! waiting, and a frame's own click says nothing of one let go. A click on a face that isn't
//! flat, or one no store can name, is refused, said in a toast in the prototype's words'
//! style, and the pick goes on; a click on nothing does nothing, as the prototype's.
//! Create Sketch again begins it again, as the prototype's does. Cancel, Esc, a dialog or a
//! sketch opened, Home or another tab shown, or the tab closing ends it.
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
/// reads are of, with the read out. A newer click supersedes one waiting.
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

    /// Whether a click waits for its face's type.
    pub fn waiting(&self) -> bool {
        self.clicked.is_some()
    }

    /// A click on `face` waiting for its type, as one made before its read answered (a
    /// test's, of what lets it go).
    #[cfg(test)]
    pub(crate) fn clicked_on(&mut self, face: Face) {
        self.clicked = Some(face);
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
        let before = (self.lit_plane(), self.lit_face().cloned());
        let frame = inputs.frame;
        let lost = self.model_is(inputs.selection.model_of());
        let mut acts = Vec::new();
        // The frame's clicks: a press and its release within egui's click distance.
        let click_px = inputs.click_distance * frame.pixels_per_point;
        let mut clicks = Vec::new();
        for event in &frame.left {
            match event.kind {
                LeftKind::Press => self.press = Some(event.at_px),
                LeftKind::Release => {
                    if self
                        .press
                        .take()
                        .is_some_and(|at| distance(at, event.at_px) <= click_px)
                    {
                        clicks.push(event.at_px);
                    }
                }
                // A stream cancelled ends with no release (`LeftKind::Cancel`): its press is
                // never a click, and the next stream's press takes its place.
                LeftKind::Cancel | LeftKind::Move | LeftKind::DoubleClick => {}
            }
        }
        let cast = |at_px| under_at(inputs, at_px);
        let taken = inputs.view.table_taken();
        acts.extend(self.clicks(&clicks, taken, lost, &cast));
        self.under = frame.pointer_px.and_then(|at_px| cast(at_px).seen());
        self.read_type(sessions, session, inputs.selection);
        acts.extend(self.answered());
        let after = (self.lit_plane(), self.lit_face().cloned());
        (acts, before != after)
    }

    /// The selection's model now `model`: if it's another than the one before, what was read
    /// of its faces let go (it's of the old one), and so is a click on a face waiting for its
    /// type, its face's name maybe another element now. Whether such a click was let go.
    fn model_is(&mut self, model: Option<(u64, Revision)>) -> bool {
        if model == self.model {
            return false;
        }
        self.model = model;
        self.typed.clear();
        self.reading = None;
        self.clicked.take().is_some()
    }

    /// The frame's clicks, at `clicks` (in physical pixels from the canvas's top left), each
    /// cast by `cast`. The newest is the one that counts: with any, a click waiting for its
    /// face's type is let go and the last of them taken. One made on another table of bodies
    /// than the one it'd be cast on (`taken`: the frame took a new table, so the frame before
    /// drew another), cast while a body's boxes are still to come, or cast on a face of meshes
    /// that aren't the selection's model's, makes nothing and says so, never held for later;
    /// an origin plane met first is taken whatever meshes the view draws (it's drawn, and
    /// named, by no model). `lost`: a click waiting for its face's type was let go as another
    /// model came this frame, said unless the frame has a click of its own.
    fn clicks(
        &mut self,
        clicks: &[[f32; 2]],
        taken: bool,
        lost: bool,
        cast: &dyn Fn([f32; 2]) -> Found,
    ) -> Option<Act> {
        let Some(at_px) = clicks.last() else {
            return lost.then(|| Act::Say(PICK_CLICK_LET_GO.to_owned()));
        };
        self.clicked = None;
        if taken {
            return Some(Act::Say(PICK_NOT_READY.to_owned()));
        }
        match cast(*at_px) {
            Found::Known(under) => self.click(under?),
            Found::Waiting(_) | Found::Stale => Some(Act::Say(PICK_NOT_READY.to_owned())),
        }
    }

    /// The face a click waits on, once its type is read: what the click asks.
    fn answered(&mut self) -> Option<Act> {
        let face = self.clicked.clone()?;
        let typed = self.typed.get(face.name.to_string().as_str()).cloned()?;
        self.clicked = None;
        decided(&face, &typed)
    }

    /// A click on `under`: a plane's sketch, or a face's once its type is known (waiting for
    /// it until then). It supersedes a click waiting on another face's type.
    fn click(&mut self, under: Under) -> Option<Act> {
        self.clicked = None;
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

/// What a toast says of a click on a face waiting for its type, let go as another model was
/// published: its face's name may be another element's now.
pub const PICK_CLICK_LET_GO: &str = "The click was let go: the model changed before what was \
                                     under it could be found; click again";

/// What a toast says of a click made while what's under it can't be known yet: made on a
/// table of bodies the view has since let go of, a body's boxes still to come, or a face met
/// first of the meshes of an older model than the one published.
pub const PICK_NOT_READY: &str = "Still preparing the model: click again";

/// What a cast of the pick finds under a pixel: what's there, known; or, while a body the
/// frame drew has its boxes still to come, what's seen there among the rest, which a body
/// not boxed yet may hide; or a face of meshes that aren't the selection's model's, met first
/// (unnamed: its number may be another element in that model).
#[derive(Debug, Clone, PartialEq)]
enum Found {
    Known(Option<Under>),
    Waiting(Option<Under>),
    Stale,
}

impl Found {
    /// What's seen there, known or not: what the pointer lights.
    fn seen(self) -> Option<Under> {
        match self {
            Found::Known(under) | Found::Waiting(under) => under,
            Found::Stale => None,
        }
    }
}

/// What's under the pixel at `at_px` (physical pixels from the canvas's top left) of
/// `inputs`' frame: the origin plane or the face its ray meets first, a face before a plane
/// within [`TIE_MM`] of it; none where neither is; [`Found::Stale`] where a face of bodies
/// not of the selection's model (not named yet) is met first. While a body drawn has its
/// boxes still to come, the same among the faces of those whose boxes are made,
/// [`Found::Waiting`] (none for such a face).
fn under_at(inputs: &Inputs<'_>, at_px: [f32; 2]) -> Found {
    let (hit, boxed) =
        match inputs
            .select_other
            .face_met(inputs.view, inputs.frame, pixel_of(inputs, at_px))
        {
            Met::Waiting(hit) => (hit, false),
            Met::Face(hit) => (Some(hit), true),
            Met::Empty => (None, true),
        };
    match nearest(inputs, at_px, hit) {
        Ok(under) if boxed => Found::Known(under),
        Ok(under) => Found::Waiting(under),
        Err(Stale) if boxed => Found::Stale,
        Err(Stale) => Found::Waiting(None),
    }
}

/// A face met first of meshes that aren't the selection's model's.
struct Stale;

/// The centre of the pixel at `at_px` of `inputs`' frame, clamped into it.
fn pixel_of(inputs: &Inputs<'_>, at_px: [f32; 2]) -> [f64; 2] {
    centre_of(square(at_px, inputs.frame.size_px, 0)).map(|c| f64::from(c) + 0.5)
}

/// Of the origin planes the frame drew and `face`, the one met first at `at_px`, if any:
/// whichever the ray through it meets first, a face before a plane within [`TIE_MM`]; or
/// [`Stale`] if it's the face and the view's meshes aren't the selection's model's.
fn nearest(
    inputs: &Inputs<'_>,
    at_px: [f32; 2],
    face: Option<silico_render::render::Hit>,
) -> Result<Option<Under>, Stale> {
    let frame = inputs.frame;
    let pixel = pixel_of(inputs, at_px);
    let plane = crate::planes::under(
        &frame.camera,
        frame.size_px,
        pixel,
        inputs.view.planes_drawn(),
    );
    let Some(hit) = face else {
        return Ok(plane.map(|(plane, _)| Under::Plane(plane)));
    };
    let Some((from, along)) = ray(&frame.camera, frame.size_px, pixel) else {
        return Ok(None);
    };
    let met = vec3::dot(vec3::sub(hit.point, from), along);
    if let Some((plane, at)) = plane
        && at + TIE_MM < met
    {
        return Ok(Some(Under::Plane(plane)));
    }
    if !inputs.current {
        return Err(Stale);
    }
    Ok(inputs
        .view
        .bodies()
        .get(&hit.body)
        .and_then(|body| body.name_of(hit.element))
        .map(|named| {
            Under::Face(Face {
                body: hit.body,
                element: hit.element,
                name: Arc::clone(&named.name),
            })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(n: u32) -> Face {
        Face {
            body: 1,
            element: Element::Face(n),
            name: Arc::new(Name::Element {
                feature: silico_core::document::FeatureId::new("k7f2qx3m").unwrap(),
                made: silico_core::naming::TypeWord::Primitive(
                    silico_core::naming::PrimitiveKind::Box,
                ),
                element: silico_protocol::ElementRef {
                    kind: silico_protocol::ElementKind::Face,
                    index: n,
                },
            }),
        }
    }

    #[test]
    fn a_newer_click_supersedes_one_waiting_for_its_faces_type() {
        // #565's review: Face5 clicked before its type is read waits; Face6 clicked next,
        // read as curved, is refused at once, and Face5's read answering later starts
        // nothing: the newest click is the one that counts.
        let mut pick = PlanePick::default();
        assert_eq!(pick.click(Under::Face(face(5))), None);
        assert!(pick.waiting());
        pick.typed.insert(
            face(6).name.to_string(),
            Typed {
                planar: Ok(false),
                stored: Ok(Arc::clone(&face(6).name)),
            },
        );
        assert_eq!(
            pick.click(Under::Face(face(6))),
            Some(Act::Say(
                "Face6 isn't flat. Pick an origin plane or a flat face to sketch on.".to_owned()
            ))
        );
        assert!(!pick.waiting(), "Face5's click let go");
        // A plane clicked supersedes one waiting too.
        assert_eq!(pick.click(Under::Face(face(5))), None);
        assert_eq!(
            pick.click(Under::Plane(OriginPlane::Xz)),
            Some(Act::Plane(OriginPlane::Xz))
        );
        assert!(!pick.waiting());
    }

    /// A pixel's cast: Face5 at x 10, a body's boxes still to come at x 20, XY at x 30, XZ at
    /// x 40, a face of an older model's meshes at x 50, nothing elsewhere.
    fn cast(at_px: [f32; 2]) -> Found {
        match at_px[0] as u32 {
            10 => Found::Known(Some(Under::Face(face(5)))),
            20 => Found::Waiting(Some(Under::Plane(OriginPlane::Xy))),
            30 => Found::Known(Some(Under::Plane(OriginPlane::Xy))),
            40 => Found::Known(Some(Under::Plane(OriginPlane::Xz))),
            50 => Found::Stale,
            _ => Found::Known(None),
        }
    }

    fn not_ready() -> Option<Act> {
        Some(Act::Say(PICK_NOT_READY.to_owned()))
    }

    #[test]
    fn a_click_before_whats_under_it_is_known_makes_nothing_and_says_so_never_held() {
        // #565's rounds 4 and 5: a click whose cast waits for a body's boxes (though XY is lit
        // there), or that meets a face of meshes that aren't the selection's model's first,
        // makes nothing, says so, and is held for nothing: the frames after it, the boxes
        // made, ask nothing. A click then on what's known there is taken.
        let mut pick = PlanePick::default();
        assert_eq!(
            pick.clicks(&[[20.0, 5.0]], false, false, &cast),
            not_ready()
        );
        assert!(!pick.waiting());
        let made = |_: [f32; 2]| Found::Known(Some(Under::Plane(OriginPlane::Xy)));
        assert_eq!(pick.clicks(&[], false, false, &made), None);
        assert_eq!(
            pick.clicks(&[[50.0, 5.0]], false, false, &cast),
            not_ready()
        );
        assert!(!pick.waiting());
        assert_eq!(pick.clicks(&[], false, false, &made), None);
        assert_eq!(
            pick.clicks(&[[30.0, 5.0]], false, false, &cast),
            Some(Act::Plane(OriginPlane::Xy))
        );
    }

    #[test]
    fn a_click_in_the_frame_a_new_table_is_taken_makes_nothing_and_says_so() {
        // #565's round 5: a click let go in the frame the view took a new table of bodies
        // was made on the table the frame before drew; cast on this one, it'd be of what the
        // person never saw. It makes nothing and says so, even over a plane known there, and
        // lets a click waiting for its face's type go; such a frame with no click says
        // nothing. The next frame's click there is taken.
        let mut pick = PlanePick::default();
        assert_eq!(pick.clicks(&[], true, false, &cast), None);
        assert_eq!(pick.clicks(&[[10.0, 5.0]], false, false, &cast), None);
        assert!(pick.waiting());
        assert_eq!(pick.clicks(&[[30.0, 5.0]], true, false, &cast), not_ready());
        assert!(!pick.waiting());
        assert_eq!(
            pick.clicks(&[[30.0, 5.0]], false, false, &cast),
            Some(Act::Plane(OriginPlane::Xy))
        );
    }

    #[test]
    fn a_frames_last_click_counts_and_lets_a_click_waiting_for_a_faces_type_go() {
        // Face5 clicked before its type is read waits. A click next frame, even one that makes
        // nothing (its cast still waiting), lets it go, so Face5's read answering asks
        // nothing. Of two clicks in one frame, the last is taken.
        let mut pick = PlanePick::default();
        assert_eq!(pick.clicks(&[[10.0, 5.0]], false, false, &cast), None);
        assert!(pick.waiting());
        assert_eq!(
            pick.clicks(&[[20.0, 5.0]], false, false, &cast),
            not_ready()
        );
        assert!(!pick.waiting());
        pick.typed.insert(
            face(5).name.to_string(),
            Typed {
                planar: Ok(true),
                stored: Ok(Arc::clone(&face(5).name)),
            },
        );
        assert_eq!(pick.answered(), None, "Face5's click let go");
        assert_eq!(
            pick.clicks(&[[30.0, 5.0], [40.0, 5.0]], false, false, &cast),
            Some(Act::Plane(OriginPlane::Xz))
        );
    }

    #[test]
    fn a_click_waiting_for_its_faces_type_is_let_go_by_another_model_said() {
        // Another model published: the face's name may be another element now, so a click
        // waiting for its type is let go, which the frame says; the same model lets go
        // nothing; and a frame with a click of its own takes it, saying nothing of the other.
        let mut pick = PlanePick::default();
        assert!(!pick.model_is(Some((1, Revision::from_number(1).unwrap()))));
        assert_eq!(pick.clicks(&[[10.0, 5.0]], false, false, &cast), None);
        assert!(!pick.model_is(Some((1, Revision::from_number(1).unwrap()))));
        assert!(pick.waiting());
        assert!(pick.model_is(Some((2, Revision::from_number(2).unwrap()))));
        assert!(!pick.waiting());
        assert_eq!(
            pick.clicks(&[], false, true, &cast),
            Some(Act::Say(PICK_CLICK_LET_GO.to_owned()))
        );
        assert_eq!(
            pick.clicks(&[[40.0, 5.0]], false, true, &cast),
            Some(Act::Plane(OriginPlane::Xz))
        );
    }
}
