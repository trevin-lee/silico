//! `render` (docs/design/m4.1-command-api.md, `render`; docs/design/m4.6-render.md, *In the
//! core*): a PNG of a model's bodies from a named view or a camera, with elements
//! highlighted, drawn by the render worker. The request is checked in M4.1's order (the
//! view or the camera, the size, the highlights, the file), then the bodies are found, each
//! highlight by its name, the camera fitted or checked (a perspective eye inside a body
//! refused), the bodies the view can't see left out, the rest meshed at about half a pixel
//! through the mesh cache, the scene checked against what one frame and one render hold,
//! and drawn. Nothing is sent to the render worker for a request refused.
//!
//! The result is a camera and sizes, never the PNG's bytes, which come beside it for the
//! client to show or write (R5: nothing the render worker returns reaches a result).

use std::path::{Path, PathBuf};

use silico_protocol::{
    Camera, MAX_FRAME_LEN, MAX_RENDER_EDGES, MAX_RENDER_FACES, MAX_RENDER_SIDE,
    MAX_RENDER_TRIANGLES, Mesh, MeshAt, PointPlace, Projection, RenderBody, RenderScene,
};

use super::lookup::{Kinds, Lookup};
use super::{ModelChoice, Problems, PublishedRead, Running, SessionId, Sessions};
use crate::document::{Angle, FeatureId, Length, Position};
use crate::file::{self, Output};
use crate::kernel::{Clients, ElementKind, Kernel, KernelRequest};
use crate::meshes::{MeshCache, mesh_level};
use crate::pool::{Outcome, Pending};
use crate::recompute::{Built, FeatureOutcome, Key, Model};
use crate::render::{RenderClient, RenderOutcome, RenderTicket};
use crate::views::{NamedView, depth_of_box, half_pixel_at, sees};
use crate::worker::SILICO_BUG;
use crate::{Error, ErrorCode};

/// The most elements one render highlights.
pub const MAX_HIGHLIGHTS: usize = 1000;

/// A render asked for (M4.1's request, its session and model aside).
#[derive(Debug, Clone, PartialEq)]
pub struct Render {
    /// A named view (`front`, `top_front_right`, ...), or none with a camera.
    pub view: Option<NamedView>,
    /// A camera, or none with a named view.
    pub camera: Option<Camera>,
    /// The image's size in pixels, each from 1 to [`MAX_RENDER_SIDE`].
    pub width: u32,
    pub height: u32,
    /// The names of the faces and edges to highlight, at most [`MAX_HIGHLIGHTS`].
    pub highlight: Vec<String>,
    /// Where to write the PNG, a `.png` path in the workspace, or none.
    pub to: Option<PathBuf>,
    /// Whether to replace a file already at `to`.
    pub replace: bool,
}

/// What a render answers (M4.1's result): the PNG's size in bytes, where it was written,
/// the image's size, and the camera it was taken with, its `up` made perpendicular to the
/// view and of unit length, so it can be passed back unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderResult {
    pub png_bytes: u64,
    pub path: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub camera: AnsweredCamera,
}

/// A camera as a result gives it, in the document's quantities: its eye and target to the
/// nanometre, its view's height to the nanometre or its angle to the nanodegree, and its up
/// a direction, made perpendicular to the view. It's the camera the render drew with
/// ([`AnsweredCamera::camera`]), so given back it draws the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnsweredCamera {
    pub projection: AnsweredProjection,
    pub eye: Position,
    pub target: Position,
    pub up: [f64; 3],
}

/// An answered camera's projection, in the document's quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsweredProjection {
    Orthographic { view_height: Length },
    Perspective { field_of_view: Angle },
}

impl AnsweredCamera {
    /// `camera` rounded as a result writes it: its eye, target, and view height to the
    /// nanometre, its angle to the nanodegree, its up as it is. None if a number doesn't fit.
    pub fn rounded(camera: &Camera) -> Option<Self> {
        let point = |p: [f64; 3]| {
            Some([
                Length::rounded(p[0])?,
                Length::rounded(p[1])?,
                Length::rounded(p[2])?,
            ])
        };
        let projection = match camera.projection {
            Projection::Orthographic { view_height } => AnsweredProjection::Orthographic {
                view_height: Length::rounded(view_height)?,
            },
            Projection::Perspective { field_of_view } => AnsweredProjection::Perspective {
                field_of_view: Angle::rounded(field_of_view)?,
            },
        };
        Some(AnsweredCamera {
            projection,
            eye: point(camera.eye)?,
            target: point(camera.target)?,
            up: camera.up,
        })
    }

    /// The camera a result answers for `camera`, as the render draws with it: rounded as
    /// a result writes it, then its up made perpendicular to the view the rounded eye and
    /// target give, so given back, rounding changes nothing and up is already its frame's.
    /// None if a number doesn't fit, or the rounded camera fails its check.
    pub fn of(camera: &Camera) -> Option<Self> {
        let rounded = Self::rounded(camera)?;
        let drawn = rounded.camera();
        drawn.check().ok()?;
        Some(AnsweredCamera {
            up: perpendicular(&drawn).up,
            ..rounded
        })
    }

    /// The camera it is, as the renderer takes it.
    pub fn camera(&self) -> Camera {
        Camera {
            projection: match self.projection {
                AnsweredProjection::Orthographic { view_height } => Projection::Orthographic {
                    view_height: view_height.mm(),
                },
                AnsweredProjection::Perspective { field_of_view } => Projection::Perspective {
                    field_of_view: field_of_view.degrees(),
                },
            },
            eye: self.eye.map(Length::mm),
            target: self.target.map(Length::mm),
            up: self.up,
        }
    }
}

/// The refusal of `count` highlights, past [`MAX_HIGHLIGHTS`]: the request's reader's and
/// the command's, in one form.
pub(crate) fn too_many_highlights(count: usize) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!("{count} elements highlighted, more than the {MAX_HIGHLIGHTS} a render highlights"),
    )
    .with_parameter("highlight")
    .with_suggestion(format!("highlight at most {MAX_HIGHLIGHTS} elements"))
}

/// A render's result, and its PNG, which is never in the result: an MCP reply's image
/// block, or a Rust client's to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendering {
    pub result: RenderResult,
    pub png: Vec<u8>,
}

/// What draws a scene: the render worker's client, or a test's fake that records what it's
/// sent.
pub trait Renderer: Send + Sync {
    /// Queues `scene`, a checked one, and gives its ticket.
    fn render(&self, scene: RenderScene) -> RenderTicket;
}

impl Renderer for RenderClient {
    fn render(&self, scene: RenderScene) -> RenderTicket {
        RenderClient::render(self, scene)
    }
}

/// A renderer shared: the sessions hold theirs so, and a test keeps a handle on its fake.
impl<R: Renderer + ?Sized> Renderer for std::sync::Arc<R> {
    fn render(&self, scene: RenderScene) -> RenderTicket {
        (**self).render(scene)
    }
}

impl<P: Clients + 'static> Sessions<P> {
    /// `render` (M4.6): a PNG of the model `model` names (decision 33), as `request` asks
    /// ([`render`]): meshed on the read's own client of the kernel, through the meshes the
    /// sessions keep, and drawn by the sessions' renderer ([`Sessions::rendering`]); the read's
    /// cancel, or its session closing, stops it, and it settles before writing its file.
    /// Refused as `worker_unavailable` if the sessions were given no renderer.
    pub fn render(
        &self,
        session: &SessionId,
        model: ModelChoice,
        request: Render,
    ) -> Running<Rendering> {
        let (renderer, meshes) = self.render_on();
        let held = self.held_paths();
        self.read_model(session, PublishedRead::Render, model, move |read| {
            let Some(renderer) = renderer else {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::WorkerUnavailable,
                        "these sessions were given no render worker",
                    )
                    .with_suggestion("start Silico with its render worker (silico-render-worker)"),
                ));
            };
            read.rendering(&*renderer, |renderer| {
                render(
                    read.model(),
                    &request,
                    &RenderOn {
                        kernel: read.kernel(),
                        meshes: &meshes,
                        renderer,
                        workspace: None,
                        held: &held,
                        settle: &|| read.settle(),
                    },
                )
            })
        })
    }
}

/// What a render runs on: a kernel client (the session's read lane), the meshes kept beside
/// the kernel, the renderer, what a file written must keep clear of (decision 35), and the
/// settle that comes before it.
pub struct RenderOn<'a, K: Kernel> {
    /// The read's kernel client, its requests at its own generation.
    pub kernel: &'a K,
    pub meshes: &'a MeshCache<Key>,
    pub renderer: &'a dyn Renderer,
    /// The workspace a file written must be inside, if the sessions have one.
    pub workspace: Option<&'a Path>,
    /// The documents the sessions hold, which a file written is never.
    pub held: &'a [PathBuf],
    /// Called just before the PNG is written, the render's one trace: refused (the read
    /// was cancelled, decision 34), nothing is written and its error is the answer. A
    /// render with no session settles at once.
    pub settle: &'a dyn Fn() -> Result<(), Error>,
}

/// The camera asked for: a named view's, fitted once the bodies are known, or one given.
enum Asked {
    Named(NamedView),
    Given(Camera),
}

/// A body of the model: its number (its place among the model's bodies, from 1), its
/// feature, its last step's key and result, and its tight box, if it has one.
struct Body<'m> {
    number: u32,
    id: &'m FeatureId,
    key: Key,
    built: &'m Built,
    bounds: Option<[[f64; 3]; 2]>,
}

/// A highlight found: the body's place in the model's bodies, and the face's or edge's
/// index from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Lit {
    body: usize,
    kind: ElementKind,
    index: u32,
}

/// Renders `model` as `request` asks, on `on` (docs/design/m4.6-render.md, *In the core*).
/// Refused, each reason named by its field, for a request that can't be drawn: sizes past
/// the limits, a view and a camera both or neither, a named view unknown or of a model with
/// no bodies, a camera that fails its check or whose perspective eye is inside a body
/// (`camera.eye`, a point on a face counting as inside), more highlights than
/// [`MAX_HIGHLIGHTS`], a highlight that names nothing (`reference_broken`, with candidates)
/// or a vertex or solid, or an element of a feature that didn't build (its failure), a `to`
/// that decision 35 refuses, and a scene past what a render draws (`camera`) or one frame
/// holds (the side whose pixels would make it coarser).
pub fn render<K: Kernel>(
    model: &Model,
    request: &Render,
    on: &RenderOn<'_, K>,
) -> Result<Rendering, Problems> {
    let _span =
        tracing::info_span!("render", width = request.width, height = request.height).entered();
    let output = Output {
        field: "to",
        extension: "png",
        replace: request.replace,
        workspace: on.workspace,
        held: on.held,
    };
    let asked = checked(request, &output)?;
    let bodies = bodies(model);
    let lit = highlighted(model, &bodies, &request.highlight)?;
    // The camera used is the one answered: a given one's up made perpendicular and of unit
    // length; a named view's as it's fitted, already so.
    let (camera, fit_side) = match asked {
        Asked::Given(camera) => (camera, None),
        Asked::Named(view) => {
            // Fitted to every body's box: one that couldn't be measured has none to fit.
            if let Some(unmeasured) = bodies.iter().find(|b| b.bounds.is_none()) {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "view {} fits the bodies by their boxes, and {}'s solid couldn't be measured for one",
                            view.name(),
                            unmeasured.id
                        ),
                    )
                    .with_feature(unmeasured.id.as_str())
                    .with_parameter("view")
                    .with_suggestion("give a camera on the bodies, or check the model's measures"),
                ));
            }
            let Some(bounds) = joined(bodies.iter().filter_map(|b| b.bounds)) else {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "view {} fits the model's bodies, and it has none to fit",
                            view.name()
                        ),
                    )
                    .with_parameter("view")
                    .with_suggestion("give a camera: one renders a model with no bodies"),
                ));
            };
            let (camera, side) = view.fit(bounds, request.width, request.height);
            if let Err(invalid) = camera.check() {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "view {} can't fit the model's bodies: {}",
                            view.name(),
                            invalid.message
                        ),
                    )
                    .with_parameter("view")
                    .with_suggestion("give a camera on the bodies"),
                ));
            }
            (camera, Some(side))
        }
    };
    // The camera answered is the one drawn, as the wire writes it (lengths to the nanometre,
    // the angle to the nanodegree), so it passes back unchanged; checked again, as rounding
    // a camera at its limits could move it past one.
    let answered = match AnsweredCamera::of(&camera) {
        Some(answered) => answered,
        _ => {
            let parameter = if fit_side.is_some() { "view" } else { "camera" };
            return Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    "the camera, its lengths to the nanometre and its angle to the nanodegree as a result writes it, can't make a view",
                )
                .with_parameter(parameter)
                .with_suggestion("give a camera within the render's limits"),
            ));
        }
    };
    let camera = answered.camera();
    let kept: Vec<&Body> = bodies
        .iter()
        // A body with no box (its measure failed) can't be shown out of view: it's kept.
        .filter(|b| {
            b.bounds.is_none_or(|bounds| {
                sees(&camera, request.width, request.height, bounds).unwrap_or(true)
            })
        })
        .collect();
    within_caps(
        kept.iter()
            .filter_map(|b| b.built.summary())
            .map(|s| (s.faces, s.edges)),
    )?;
    eye_outside(&camera, &bodies, on)?;
    let meshes = meshed(&camera, request.height, &kept, on)?;
    // Again by the meshes: a body unmeasured wasn't counted before.
    within_caps(meshes.iter().map(|m| {
        (
            u32::try_from(m.face_triangles.len()).unwrap_or(u32::MAX),
            u32::try_from(m.edge_point_counts.len()).unwrap_or(u32::MAX),
        )
    }))?;
    // And their triangles, before any is copied into the scene: fewer pixels mesh coarser.
    within_triangles(&meshes, bodies.len() > kept.len(), fit_side)?;
    let scene = RenderScene {
        bodies: kept
            .iter()
            .zip(meshes)
            .map(|(body, mesh)| body_of(body, mesh, &lit))
            .collect::<Result<_, _>>()?,
        camera,
        width: request.width,
        height: request.height,
        ids: false,
    };
    if let Err(invalid) = scene.check() {
        // A camera's own numbers were checked: what's left is its zoom against the bodies
        // it sees, which the request can change. Anything else is the core's mesh.
        let error = match invalid.parameter {
            parameter if parameter.starts_with("camera.") && fit_side.is_none() => {
                Error::new(ErrorCode::InvalidInput, invalid.message)
                    .with_parameter(parameter)
                    .with_suggestion("change the value the message names")
            }
            parameter if parameter.starts_with("camera.") => Error::new(
                ErrorCode::InvalidInput,
                format!("the view fitted can't draw the bodies: {}", invalid.message),
            )
            .with_parameter("view")
            .with_suggestion("give a camera on the bodies"),
            parameter => Error::new(
                ErrorCode::Internal,
                "the core built a scene the render worker can't draw",
            )
            .with_parameter(parameter)
            .with_cause(invalid.message)
            .with_suggestion(SILICO_BUG),
        };
        return Err(Problems::from(error));
    }
    within_a_frame(
        &scene,
        bodies.len() > kept.len(),
        fit_side,
        u64::from(MAX_FRAME_LEN),
    )?;
    let png = match on.renderer.render(scene).wait() {
        RenderOutcome::Done(result) => result.map_err(Problems::from)?.png,
        // Stopped by its read's stop, its queued scene never sent or its worker killed: a
        // cancel's answer the session gives (`cancelled`), so this is a close's.
        RenderOutcome::Cancelled => return Err(Problems::from(session_closed())),
    };
    let path = match &request.to {
        Some(to) => {
            (on.settle)().map_err(Problems::from)?;
            Some(file::write_output(to, &png, &output).map_err(Problems::from)?)
        }
        None => None,
    };
    Ok(Rendering {
        result: RenderResult {
            png_bytes: png.len() as u64,
            path,
            width: request.width,
            height: request.height,
            camera: answered,
        },
        png,
    })
}

/// The request's own values checked, in M4.1's order, every reason it's refused for found:
/// the view or the camera, the size, the highlights' count, and `to`.
fn checked(request: &Render, output: &Output) -> Result<Asked, Problems> {
    let mut errors = Vec::new();
    let invalid = |parameter: &str, message: String, suggestion: &str| {
        Error::new(ErrorCode::InvalidInput, message)
            .with_parameter(parameter)
            .with_suggestion(suggestion)
    };
    let mut asked = None;
    match (&request.view, &request.camera) {
        (Some(_), Some(_)) => errors.push(invalid(
            "camera",
            "give a named view or a camera, not both".to_owned(),
            "leave out the camera to take the named view, or the view to take the camera",
        )),
        (None, None) => errors.push(invalid(
            "view",
            "give a named view or a camera".to_owned(),
            "give a view, such as top_front_right, or a camera",
        )),
        (Some(view), None) => asked = Some(Asked::Named(*view)),
        (None, Some(camera)) => match camera.check() {
            Ok(()) => asked = Some(Asked::Given(camera.clone())),
            Err(e) => errors.push(invalid(
                e.parameter,
                e.message,
                "change the value the message names",
            )),
        },
    }
    for (parameter, side) in [("width", request.width), ("height", request.height)] {
        if !(1..=MAX_RENDER_SIDE).contains(&side) {
            errors.push(invalid(
                parameter,
                format!("{parameter} must be from 1 to {MAX_RENDER_SIDE} pixels, got {side}"),
                &format!("give a {parameter} from 1 to {MAX_RENDER_SIDE}"),
            ));
        }
    }
    if request.highlight.len() > MAX_HIGHLIGHTS {
        errors.push(too_many_highlights(request.highlight.len()));
    }
    if let Some(to) = &request.to
        && let Err(e) = file::check_output(to, output)
    {
        errors.push(e);
    }
    match asked {
        Some(asked) if errors.is_empty() => Ok(asked),
        _ => Err(Problems::of(errors)),
    }
}

/// The model's bodies, in its order, each numbered by its place among them from 1.
fn bodies(model: &Model) -> Vec<Body<'_>> {
    model
        .bodies()
        .iter()
        .filter_map(|id| match model.outcome(id) {
            Some(FeatureOutcome::Built { key, built }) => Some((id, *key, built.as_ref())),
            _ => None,
        })
        .zip(1u32..)
        .map(|((id, key, built), number)| Body {
            number,
            id,
            key,
            built,
            bounds: built.summary().and_then(|s| s.bounds),
        })
        .collect()
}

/// The box around `boxes`, if there are any.
fn joined(boxes: impl Iterator<Item = [[f64; 3]; 2]>) -> Option<[[f64; 3]; 2]> {
    boxes.reduce(|[low, high], [l, h]| {
        [
            [low[0].min(l[0]), low[1].min(l[1]), low[2].min(l[2])],
            [high[0].max(h[0]), high[1].max(h[1]), high[2].max(h[2])],
        ]
    })
}

/// What a render highlights by name: a face or an edge, which have pixels of their own.
const HIGHLIGHTED: Kinds = Kinds {
    taken: &[ElementKind::Face, ElementKind::Edge],
    why: "only a face or an edge has pixels of its own to highlight",
    suggestion: "highlight the faces or edges around it",
};

/// Each name in `highlight` found in the model's bodies by the lookup every command that
/// reads elements by name shares ([`Lookup`]), each naming `highlight.N`: one that isn't a
/// name, or names nothing, `reference_broken` with the likeliest candidates; a vertex or a
/// solid `invalid_input`; an element of a feature that didn't build, that feature's
/// failure. An unstable name is taken, as a read takes one (M4.1 section 2). Every one's
/// error is found, the first [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED) listed.
fn highlighted(model: &Model, bodies: &[Body], highlight: &[String]) -> Result<Vec<Lit>, Problems> {
    if highlight.is_empty() {
        return Ok(Vec::new());
    }
    let mut lookup = Lookup::new(model, None, HIGHLIGHTED);
    let mut lit = Vec::new();
    for (i, text) in highlight.iter().enumerate() {
        // Which element only: a body is drawn, and its faces lit, though its measure failed.
        let Some((feature, element)) = lookup.find_element(&format!("highlight.{i}"), text) else {
            continue;
        };
        // A solid the lookup finds is one of the bodies that built, which are drawn.
        match bodies.iter().position(|b| b.id == feature) {
            Some(body) => lit.push(Lit {
                body,
                kind: element.kind,
                index: element.index.saturating_sub(1),
            }),
            None => lookup.refuse(
                Error::new(
                    ErrorCode::Internal,
                    format!(
                        "highlight.{i} was found in {feature}'s solid, which isn't among the bodies drawn"
                    ),
                )
                .with_parameter(format!("highlight.{i}"))
                .with_suggestion(SILICO_BUG),
            ),
        }
    }
    lookup.finish()?;
    lit.sort();
    lit.dedup();
    Ok(lit)
}

/// Refuses bodies in view with more faces or edges than a render draws, by each body's
/// counts: their B-reps' before any is meshed, and their meshes' after.
fn within_caps(counts: impl Iterator<Item = (u32, u32)>) -> Result<(), Problems> {
    let (faces, edges) = counts.fold((0u64, 0u64), |(f, e), (bf, be)| {
        (f + u64::from(bf), e + u64::from(be))
    });
    for (what, count, most) in [
        ("faces", faces, MAX_RENDER_FACES),
        ("edges", edges, MAX_RENDER_EDGES),
    ] {
        if count > most as u64 {
            return Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "the bodies in view have {count} {what}, more than the {most} a render draws"
                    ),
                )
                .with_parameter("camera")
                .with_suggestion("give a camera on part of the model, so fewer bodies are in view"),
            ));
        }
    }
    Ok(())
}

/// Refuses meshes of more triangles, across the bodies in view, than a render draws
/// ([`MAX_RENDER_TRIANGLES`]), naming the side whose fewer pixels would mesh them coarser,
/// as [`coarser`] says.
fn within_triangles(
    meshes: &[std::sync::Arc<Mesh>],
    left_out: bool,
    fit_side: Option<&'static str>,
) -> Result<(), Problems> {
    let count: u64 = meshes.iter().map(|m| m.triangles.len() as u64).sum();
    if count <= MAX_RENDER_TRIANGLES as u64 {
        return Ok(());
    }
    let (parameter, suggestion) = coarser(left_out, fit_side);
    Err(Problems::from(
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the bodies in view mesh to {count} triangles at this view's pixel, more than the {MAX_RENDER_TRIANGLES} a render draws"
            ),
        )
        .with_parameter(parameter)
        .with_suggestion(suggestion),
    ))
}

/// The field whose smaller value meshes coarser, and the advice: a named view's side that
/// bound its fit, or a camera's `height`, with a wider view too where no body was left out
/// (where some were, a wider view brings them in, and the scene can grow).
fn coarser(left_out: bool, fit_side: Option<&'static str>) -> (&'static str, String) {
    match fit_side {
        Some(side) => (
            side,
            format!("give a smaller {side}: it meshes the bodies coarser"),
        ),
        None if left_out => (
            "height",
            "give a smaller height: it meshes the bodies coarser".to_owned(),
        ),
        None => (
            "height",
            "give a smaller height, or a wider view: either meshes the bodies coarser".to_owned(),
        ),
    }
}

/// Refuses a perspective camera whose eye is inside a body or on its boundary, by the
/// kernel's classifier, asked only of the bodies whose tight box holds the eye. An
/// orthographic view uses only the eye's direction, so where its eye is doesn't matter.
fn eye_outside<K: Kernel>(
    camera: &Camera,
    bodies: &[Body],
    on: &RenderOn<'_, K>,
) -> Result<(), Problems> {
    if !matches!(camera.projection, Projection::Perspective { .. }) {
        return Ok(());
    }
    let eye = camera.eye;
    let asked: Vec<(&Body, Pending<PointPlace>)> = bodies
        .iter()
        // A body with no box (its measure failed) is asked too.
        .filter(|b| {
            b.bounds
                .is_none_or(|[low, high]| (0..3).all(|k| low[k] <= eye[k] && eye[k] <= high[k]))
        })
        .map(|b| {
            let request = KernelRequest::classify(&b.built.shape, eye);
            (b, on.kernel.submit(on.kernel.generation(), request))
        })
        .collect();
    for (body, pending) in asked {
        // On its boundary counts as inside (the design: an eye on a face is refused too).
        if done(pending.wait()).map_err(|p| naming_the_body(p, body.id))? != PointPlace::Outside {
            return Err(Problems::from(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!("the camera's eye is inside {}'s solid", body.id),
                )
                .with_feature(body.id.as_str())
                .with_parameter("camera.eye")
                .with_suggestion("move the eye outside the part, or use a named view"),
            ));
        }
    }
    Ok(())
}

/// The bodies in view meshed at about half a pixel (`mesh_level`), each from the cache if
/// it's there, the rest asked of the kernel at once and kept as they come.
fn meshed<K: Kernel>(
    camera: &Camera,
    height: u32,
    kept: &[&Body],
    on: &RenderOn<'_, K>,
) -> Result<Vec<std::sync::Arc<Mesh>>, Problems> {
    if camera.frame().is_err() {
        return Err(Problems::from(
            Error::new(
                ErrorCode::Internal,
                "the camera checked has no size of pixel to mesh to",
            )
            .with_suggestion(SILICO_BUG),
        ));
    }
    // Half a pixel at the body's nearest depth along the view (its box's; a point's pixel
    // scales with its depth, not its distance), its power of two; at no depth (a box reaching
    // the eye's plane, or none), the finest there is, which meshes at the floor.
    let level = |b: &Body| {
        let depth = b.bounds.map_or(0.0, |bounds| depth_of_box(camera, bounds));
        half_pixel_at(camera, height, depth)
            .and_then(mesh_level)
            .unwrap_or(f64::MIN_POSITIVE)
    };
    enum Got {
        Kept(std::sync::Arc<Mesh>),
        Asked(f64, Pending<MeshAt>),
    }
    let got: Vec<Got> = kept
        .iter()
        .map(|b| {
            let deflection = on.meshes.deflection_for(&b.key, level(b));
            match on.meshes.get(&b.key, deflection) {
                Some(mesh) => Got::Kept(mesh),
                None => Got::Asked(
                    deflection,
                    on.kernel.submit(
                        on.kernel.generation(),
                        KernelRequest::tessellate_at_least_the_floor(&b.built.shape, deflection),
                    ),
                ),
            }
        })
        .collect();
    kept.iter()
        .zip(got)
        .map(|(b, got)| match got {
            Got::Kept(mesh) => Ok(mesh),
            Got::Asked(asked, pending) => {
                let made = done(pending.wait()).map_err(|p| naming_the_body(p, b.id))?;
                Ok(on.meshes.put(&b.key, asked, made))
            }
        })
        .collect()
}

/// A kernel request's result: its error as the refusal, and one superseded as its session
/// closing.
fn done<T>(outcome: Outcome<T>) -> Result<T, Problems> {
    match outcome {
        Outcome::Done(result) => result.map_err(Problems::from),
        // A read's own client is advanced only when its read is stopped: a cancel, whose
        // answer the session gives (`cancelled`), or its session closing, this.
        Outcome::Superseded => Err(Problems::from(session_closed())),
    }
}

/// The answer of a render stopped by its session closing, as `distance` answers one:
/// a published read answers as of a session that closed, never `cancelled` (decision 34).
fn session_closed() -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        "the session closed before the render finished",
    )
    .with_parameter("session")
    .with_suggestion("open the document again, and render it there")
}

/// The scene's body for `body`: its number, its mesh, and its faces and edges `lit`, each
/// checked against the mesh (an element map and its mesh are of one shape).
fn body_of(body: &Body, mesh: std::sync::Arc<Mesh>, lit: &[Lit]) -> Result<RenderBody, Problems> {
    // Numbered by its place among the bodies, from 1.
    let place = (body.number as usize).saturating_sub(1);
    let of = |kind: ElementKind, count: usize| -> Result<Vec<u32>, Problems> {
        lit.iter()
            .filter(|l| l.body == place && l.kind == kind)
            .map(|l| {
                if (l.index as usize) < count {
                    Ok(l.index)
                } else {
                    Err(Problems::from(
                        Error::new(
                            ErrorCode::Internal,
                            format!(
                                "{}'s element map names {} {}, past its mesh's {count}",
                                body.id,
                                a_kind(kind),
                                l.index + 1
                            ),
                        )
                        .with_suggestion(SILICO_BUG),
                    ))
                }
            })
            .collect()
    };
    let highlight_faces = of(ElementKind::Face, mesh.face_triangles.len())?;
    let highlight_edges = of(ElementKind::Edge, mesh.edge_point_counts.len())?;
    Ok(RenderBody {
        body: body.number,
        mesh: Mesh::clone(&mesh),
        highlight_faces,
        highlight_edges,
    })
}

/// Refuses a scene whose encoding would pass the frame limit, by an upper bound on its
/// size (each number at its widest), naming the side whose fewer pixels would make its
/// meshes coarser: a camera's `height`, or a named view's side that bound its fit. Only
/// where no body was left out of the view does a camera's advice add a wider view.
fn within_a_frame(
    scene: &RenderScene,
    left_out: bool,
    fit_side: Option<&'static str>,
    most: u64,
) -> Result<(), Problems> {
    let bound = frame_bound(scene);
    if bound <= most {
        return Ok(());
    }
    let (parameter, suggestion) = coarser(left_out, fit_side);
    Err(Problems::from(
        Error::new(
            ErrorCode::InvalidInput,
            format!(
                "the scene's meshes would take up to {bound} bytes, more than the {most} one message to the render worker holds"
            ),
        )
        .with_parameter(parameter)
        .with_suggestion(suggestion),
    ))
}

/// The most bytes `scene` encodes to: every f64 8, every f32 4, every whole number at a
/// varint's widest, 10, and a list's length a whole number, with room for the rest.
fn frame_bound(scene: &RenderScene) -> u64 {
    let n = |len: usize, each: u64| (len as u64).saturating_mul(each).saturating_add(10);
    scene
        .bodies
        .iter()
        .map(|b| {
            let m = &b.mesh;
            [
                n(m.positions.len(), 24),
                n(m.normals.len(), 12),
                n(m.triangles.len(), 30),
                n(m.face_triangles.len(), 10),
                n(m.edge_points.len(), 24),
                n(m.edge_point_counts.len(), 10),
                n(b.highlight_faces.len(), 10),
                n(b.highlight_edges.len(), 10),
                64,
            ]
            .into_iter()
            .fold(0, u64::saturating_add)
        })
        .fold(1024, u64::saturating_add)
}

/// `camera`, its up made perpendicular to the view and of unit length, by its frame: the
/// camera a result gives, which passed back draws the same.
fn perpendicular(camera: &Camera) -> Camera {
    // The frame's up, taken again until the frame gives it back to within 1e-12 a part (its
    // own rounding: an up nearly along the view loses digits to the projection), and that up
    // kept as it is: so an answer passed back is answered to the bit.
    let mut up = camera.up;
    for _ in 0..4 {
        let with = Camera {
            up,
            ..camera.clone()
        };
        match with.frame() {
            Ok(frame) if (0..3).all(|k| (frame.up[k] - up[k]).abs() <= 1e-12) => break,
            Ok(frame) => up = frame.up,
            Err(_) => break,
        }
    }
    Camera {
        up,
        ..camera.clone()
    }
}

/// `a face`, `an edge`, `a vertex`, `a solid`.
fn a_kind(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Face => "a face",
        ElementKind::Edge => "an edge",
        ElementKind::Vertex => "a vertex",
        ElementKind::Solid => "a solid",
    }
}

/// `problems`, each naming `feature` if it names none: a body's kernel request's.
fn naming_the_body(problems: Problems, feature: &FeatureId) -> Problems {
    let total = problems.total();
    let errors: Vec<Error> = problems
        .errors()
        .iter()
        // A session's close isn't the body's.
        .map(|e| match (e.feature(), e.parameter()) {
            (Some(_), _) | (None, Some("session")) => e.clone(),
            (None, _) => e.clone().with_feature(feature.as_str()),
        })
        .collect();
    let listed = errors.len() as u64;
    Problems::with_unlisted(errors, total - listed)
}

#[cfg(test)]
mod tests {
    use super::{frame_bound, within_a_frame};
    use silico_protocol::{Camera, Mesh, Projection, RenderBody, RenderScene};

    /// A scene of one body: a triangle, one face, no edges, a face highlighted.
    fn one_triangle() -> RenderScene {
        RenderScene {
            bodies: vec![RenderBody {
                body: 1,
                mesh: Mesh {
                    positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    normals: vec![[0.0, 0.0, 1.0]; 3],
                    triangles: vec![[0, 1, 2]],
                    face_triangles: vec![1],
                    edge_points: vec![],
                    edge_point_counts: vec![],
                    valid: None,
                },
                highlight_faces: vec![0],
                highlight_edges: vec![],
            }],
            camera: Camera {
                projection: Projection::Orthographic { view_height: 2.0 },
                eye: [0.0, 0.0, 10.0],
                target: [0.0; 3],
                up: [0.0, 1.0, 0.0],
            },
            width: 10,
            height: 10,
            ids: false,
        }
    }

    #[test]
    fn a_scenes_bound_counts_each_number_at_its_widest() {
        // By hand: positions 3 × 24 + 10, normals 3 × 12 + 10, a triangle 30 + 10, a face
        // 10 + 10, no edge points or counts 10 + 10, a highlighted face 10 + 10 and none
        // 10, the body's 64, and the scene's 1,024.
        let want = 82 + 46 + 40 + 20 + 10 + 10 + 20 + 10 + 64 + 1024;
        assert_eq!(frame_bound(&one_triangle()), want);
        // Never under the scene's encoding.
        let encoded = postcard::to_allocvec(&one_triangle()).unwrap().len() as u64;
        assert!(encoded <= want, "{encoded} over {want}");
    }

    #[test]
    fn a_scene_past_a_frame_names_the_side_that_makes_its_meshes_coarser() {
        let scene = one_triangle();
        let bound = frame_bound(&scene);
        assert!(within_a_frame(&scene, false, None, bound).is_ok());
        for (left_out, fit, field, wider) in [
            (false, None, "height", true),
            (true, None, "height", false),
            (false, Some("width"), "width", false),
            (true, Some("height"), "height", false),
        ] {
            let problems = within_a_frame(&scene, left_out, fit, bound - 1).unwrap_err();
            let e = &problems.errors()[0];
            assert_eq!(e.code(), crate::ErrorCode::InvalidInput);
            assert_eq!(e.parameter(), Some(field), "{e:?}");
            let suggestion = e.suggestion().unwrap_or_default();
            assert_eq!(suggestion.contains("wider view"), wider, "{suggestion}");
            assert!(
                e.message().contains(&format!("up to {bound} bytes")),
                "{e:?}"
            );
        }
    }
}
