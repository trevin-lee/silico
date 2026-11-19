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
//! the same; one while a body drawn has its boxes still to come (made off the UI thread as
//! the view takes a mesh) waits for them and is cast again then, unless the view moves or
//! another model is published first (said), as the selection keeps a click; meanwhile the pointer lights what it meets among the
//! bodies whose boxes are made. A newer click supersedes one waiting. A click on a face
//! that isn't flat, or one no store can name, is refused, said in a toast in the prototype's
//! words' style, and the pick goes on; a click on nothing does nothing, as the prototype's.
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
/// waiting for the boxes of a body the frame drew (its place, cast again once they're made),
/// a click on a face waiting for the face's type, and each face read for its type on the
/// model its reads are of, with the read out. A newer click supersedes either one waiting.
#[derive(Default)]
pub struct PlanePick {
    under: Option<Under>,
    press: Option<[f32; 2]>,
    held: Option<Held>,
    clicked: Option<Face>,
    typed: BTreeMap<String, Typed>,
    model: Option<(u64, Revision)>,
    read: Measures,
    reading: Option<Reading>,
    asked: u64,
}

/// A click waiting for the boxes of a body its frame drew: where it was let go, in physical
/// pixels from the canvas's top left, and the camera and size it was let go under, so a
/// view moved since lets it go (its place was of the view before), as the selection lets a
/// click go.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    at_px: [f32; 2],
    camera: silico_protocol::Camera,
    size_px: [u32; 2],
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

    /// Whether a click waits: for the boxes to cast it on, or for its face's type.
    pub fn waiting(&self) -> bool {
        self.held.is_some() || self.clicked.is_some()
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
        let mut acts: Vec<Act> = self
            .model_is(inputs.selection.model_of())
            .into_iter()
            .collect();
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
        acts.extend(self.clicks(&clicks, &frame.camera, frame.size_px, &cast));
        self.under = frame.pointer_px.and_then(|at_px| cast(at_px).seen());
        self.read_type(sessions, session, inputs.selection);
        acts.extend(self.answered());
        let after = (self.lit_plane(), self.lit_face().cloned());
        (acts, before != after)
    }

    /// The selection's model now `model`: if it's another than the one before, what was read
    /// of its faces let go (it's of the old one), and so is a click on a face waiting for its
    /// type; and a click held for the boxes, whose place would be cast on the new model's
    /// meshes, not those it was aimed at, as the selection lets a click go
    /// (`selection::CLICK_REPUBLISHED`), said.
    fn model_is(&mut self, model: Option<(u64, Revision)>) -> Option<Act> {
        if model == self.model {
            return None;
        }
        self.model = model;
        self.typed.clear();
        self.clicked = None;
        self.reading = None;
        self.held
            .take()
            .map(|_| Act::Say(CLICK_REPUBLISHED.to_owned()))
    }

    /// The frame's clicks, at `clicks` (in physical pixels from the canvas's top left), in a
    /// view from `camera` `size_px` in pixels, each cast by `cast`. The newest is the one that
    /// counts: with any, a click waiting (for the boxes, or for its face's type) is let go and
    /// the last of them taken; with none, a click held for the boxes is cast again (let go if
    /// the view moved since). One cast while the boxes are still to come is held for them.
    fn clicks(
        &mut self,
        clicks: &[[f32; 2]],
        camera: &silico_protocol::Camera,
        size_px: [u32; 2],
        cast: &dyn Fn([f32; 2]) -> Found,
    ) -> Option<Act> {
        let at_px = match clicks.last() {
            Some(at_px) => {
                self.held = None;
                self.clicked = None;
                *at_px
            }
            None => {
                let held = self.held.take()?;
                if held.camera != *camera || held.size_px != size_px {
                    return None;
                }
                held.at_px
            }
        };
        match cast(at_px) {
            Found::Waiting(_) => {
                self.held = Some(Held {
                    at_px,
                    camera: camera.clone(),
                    size_px,
                });
                None
            }
            Found::Known(under) => self.click(under?),
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

/// What a toast says of a click held for the boxes let go as another model was published:
/// what it would be picked on isn't what it was aimed at.
pub const CLICK_REPUBLISHED: &str = "The click was let go: the model changed before what was \
                                     under it could be found; click again";

/// What a cast of the pick finds under a pixel: what's there, known; or, while a body the
/// frame drew has its boxes still to come, what's seen there among the rest, which a body
/// not boxed yet may hide.
#[derive(Debug, Clone, PartialEq)]
enum Found {
    Known(Option<Under>),
    Waiting(Option<Under>),
}

impl Found {
    /// What's seen there, known or not: what the pointer lights.
    fn seen(self) -> Option<Under> {
        match self {
            Found::Known(under) | Found::Waiting(under) => under,
        }
    }
}

/// What's under the pixel at `at_px` (physical pixels from the canvas's top left) of
/// `inputs`' frame: the origin plane or the face its ray meets first, a face before a plane
/// within [`TIE_MM`] of it; none where neither is, or where a face of bodies not of the
/// selection's model (not named yet) is met first. While a body drawn has its boxes still to
/// come, the same among the faces of those whose boxes are made, [`Found::Waiting`].
fn under_at(inputs: &Inputs<'_>, at_px: [f32; 2]) -> Found {
    match inputs
        .select_other
        .face_met(inputs.view, inputs.frame, pixel_of(inputs, at_px))
    {
        Met::Waiting(hit) => Found::Waiting(nearest(inputs, at_px, hit)),
        Met::Face(hit) => Found::Known(nearest(inputs, at_px, Some(hit))),
        Met::Empty => Found::Known(nearest(inputs, at_px, None)),
    }
}

/// The centre of the pixel at `at_px` of `inputs`' frame, clamped into it.
fn pixel_of(inputs: &Inputs<'_>, at_px: [f32; 2]) -> [f64; 2] {
    centre_of(square(at_px, inputs.frame.size_px, 0)).map(|c| f64::from(c) + 0.5)
}

/// Of the origin planes the frame drew and `face`, the face met first at `at_px`, if any:
/// whichever the ray through it meets first, a face before a plane within [`TIE_MM`].
fn nearest(
    inputs: &Inputs<'_>,
    at_px: [f32; 2],
    face: Option<silico_render::render::Hit>,
) -> Option<Under> {
    let frame = inputs.frame;
    let pixel = pixel_of(inputs, at_px);
    let plane = crate::planes::under(
        &frame.camera,
        frame.size_px,
        pixel,
        inputs.view.planes_drawn(),
    );
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

    /// A camera from above, and the size of its image, for `clicks`.
    fn above() -> (silico_protocol::Camera, [u32; 2]) {
        (
            silico_protocol::Camera {
                projection: silico_protocol::Projection::Orthographic { view_height: 40.0 },
                eye: [20.0, 10.0, 100.0],
                target: [20.0, 10.0, 0.0],
                up: [0.0, 1.0, 0.0],
            },
            [400, 300],
        )
    }

    /// A pixel's cast: Face5 at x 10, the boxes still to come at x 20, XY at x 30, XZ at
    /// x 40, nothing elsewhere.
    fn cast(at_px: [f32; 2]) -> Found {
        match at_px[0] as u32 {
            10 => Found::Known(Some(Under::Face(face(5)))),
            20 => Found::Waiting(None),
            30 => Found::Known(Some(Under::Plane(OriginPlane::Xy))),
            40 => Found::Known(Some(Under::Plane(OriginPlane::Xz))),
            _ => Found::Known(None),
        }
    }

    #[test]
    fn a_click_held_for_the_boxes_lets_a_click_waiting_for_a_faces_type_go() {
        // #565's round 2: Face5 clicked before its type is read waits; a click next frame
        // where the boxes are still to come is held for them, and lets Face5's go, so Face5's
        // read answering (a plane, stored) asks nothing. The held click then cast once the
        // boxes are made, on XY, is the one taken.
        let (camera, size) = above();
        let mut pick = PlanePick::default();
        assert_eq!(pick.clicks(&[[10.0, 5.0]], &camera, size, &cast), None);
        assert_eq!(pick.clicked, Some(face(5)));
        assert_eq!(pick.clicks(&[[20.0, 5.0]], &camera, size, &cast), None);
        assert_eq!(pick.clicked, None);
        assert!(pick.held.is_some());
        pick.typed.insert(
            face(5).name.to_string(),
            Typed {
                planar: Ok(true),
                stored: Ok(Arc::clone(&face(5).name)),
            },
        );
        assert_eq!(pick.answered(), None, "Face5's click let go");
        let made = |at_px: [f32; 2]| match at_px[0] as u32 {
            20 => Found::Known(Some(Under::Plane(OriginPlane::Xy))),
            _ => cast(at_px),
        };
        assert_eq!(
            pick.clicks(&[], &camera, size, &made),
            Some(Act::Plane(OriginPlane::Xy))
        );
        assert!(!pick.waiting());
    }

    #[test]
    fn a_click_in_the_frame_a_held_click_could_be_cast_again_is_the_one_taken() {
        // The newest wins: a click held for the boxes, and in the frame they're made a new
        // click on XZ; only XZ is asked, never the held click's XY. Of two clicks in one
        // frame, the last.
        let (camera, size) = above();
        let mut pick = PlanePick::default();
        assert_eq!(pick.clicks(&[[20.0, 5.0]], &camera, size, &cast), None);
        let made = |at_px: [f32; 2]| match at_px[0] as u32 {
            20 => Found::Known(Some(Under::Plane(OriginPlane::Xy))),
            _ => cast(at_px),
        };
        assert_eq!(
            pick.clicks(&[[40.0, 5.0]], &camera, size, &made),
            Some(Act::Plane(OriginPlane::Xz))
        );
        assert!(!pick.waiting());
        assert_eq!(
            pick.clicks(&[[30.0, 5.0], [40.0, 5.0]], &camera, size, &cast),
            Some(Act::Plane(OriginPlane::Xz))
        );
    }

    #[test]
    fn a_click_held_for_the_boxes_is_let_go_by_a_view_moved_or_another_model_said() {
        // As the selection lets a click go: the camera moved since (its place was of the view
        // before), silently; another model published (what it would be cast on isn't what it
        // was aimed at), said in the selection's way.
        let (camera, size) = above();
        let mut moved = camera.clone();
        moved.eye[0] += 1.0;
        let made = |_: [f32; 2]| Found::Known(Some(Under::Plane(OriginPlane::Xy)));
        let mut pick = PlanePick::default();
        assert_eq!(pick.clicks(&[[20.0, 5.0]], &camera, size, &cast), None);
        assert_eq!(pick.clicks(&[], &moved, size, &made), None);
        assert!(!pick.waiting());
        assert_eq!(
            pick.model_is(Some((1, Revision::from_number(1).unwrap()))),
            None
        );
        assert_eq!(pick.clicks(&[[20.0, 5.0]], &camera, size, &cast), None);
        assert_eq!(
            pick.model_is(Some((1, Revision::from_number(1).unwrap()))),
            None,
            "the same model"
        );
        assert!(pick.waiting());
        assert_eq!(
            pick.model_is(Some((2, Revision::from_number(2).unwrap()))),
            Some(Act::Say(CLICK_REPUBLISHED.to_owned()))
        );
        assert!(!pick.waiting());
        assert_eq!(pick.clicks(&[], &camera, size, &made), None);
    }
}
