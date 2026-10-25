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
    Camera, MAX_FRAME_LEN, MAX_RENDER_EDGES, MAX_RENDER_FACES, MAX_RENDER_SIDE, Mesh, MeshAt,
    Projection, RenderBody, RenderScene,
};

use super::Problems;
use crate::document::{FeatureId, PROBLEMS_LISTED};
use crate::file::{self, Output};
use crate::kernel::{ElementKind, Generation, Kernel, KernelRequest};
use crate::meshes::{MeshCache, mesh_level};
use crate::naming::{ElementMap, Maker, Name};
use crate::pool::{Outcome, Pending};
use crate::recompute::{Built, FeatureOutcome, Key, Model};
use crate::render::{RenderClient, RenderOutcome, RenderTicket};
use crate::views::{NAMED_VIEWS, NamedView, half_pixel, sees};
use crate::worker::SILICO_BUG;
use crate::{Error, ErrorCode};

/// The most elements one render highlights.
pub const MAX_HIGHLIGHTS: usize = 1000;

/// A render asked for (M4.1's request, its session and model aside).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderRequest {
    /// A named view (`front`, `top_front_right`, ...), or none with a camera.
    pub view: Option<String>,
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
    pub camera: Camera,
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

/// What a render runs on: a kernel client and the generation its requests are for (the
/// session's read lane), the meshes kept beside the kernel, the renderer, and what a file
/// written must keep clear of (decision 35).
pub struct RenderOn<'a, K: Kernel> {
    pub kernel: &'a K,
    pub generation: Generation,
    pub meshes: &'a MeshCache<Key>,
    pub renderer: &'a dyn Renderer,
    /// The workspace a file written must be inside, if the sessions have one.
    pub workspace: Option<&'a Path>,
    /// The documents the sessions hold, which a file written is never.
    pub held: &'a [PathBuf],
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
/// or a vertex or solid, a `to` that decision 35 refuses, and a scene past what a render
/// draws (`camera`) or one frame holds (the side whose pixels would make it coarser).
pub fn render<K: Kernel>(
    model: &Model,
    request: &RenderRequest,
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
        Asked::Given(camera) => (perpendicular(&camera), None),
        Asked::Named(view) => {
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
    let kept: Vec<&Body> = bodies
        .iter()
        .filter(|b| {
            b.bounds.is_some_and(|bounds| {
                sees(&camera, request.width, request.height, bounds).unwrap_or(true)
            })
        })
        .collect();
    within_caps(&kept)?;
    eye_outside(&camera, &bodies, on)?;
    let meshes = meshed(&camera, request.height, &kept, on)?;
    let taken = camera.clone();
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
        RenderOutcome::Cancelled => {
            return Err(Problems::from(
                Error::new(
                    ErrorCode::Internal,
                    "the render was cancelled before it answered",
                )
                .with_suggestion("render again"),
            ));
        }
    };
    let path = match &request.to {
        Some(to) => Some(file::write_output(to, &png, &output).map_err(Problems::from)?),
        None => None,
    };
    Ok(Rendering {
        result: RenderResult {
            png_bytes: png.len() as u64,
            path,
            width: request.width,
            height: request.height,
            camera: taken,
        },
        png,
    })
}

/// The request's own values checked, in M4.1's order, every reason it's refused for found:
/// the view or the camera, the size, the highlights' count, and `to`.
fn checked(request: &RenderRequest, output: &Output) -> Result<Asked, Problems> {
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
        (Some(view), None) => match NamedView::named(view) {
            Some(named) => asked = Some(Asked::Named(named)),
            None => {
                let names: Vec<&str> = NAMED_VIEWS.iter().map(|v| v.name()).collect();
                errors.push(invalid(
                    "view",
                    format!(
                        "view {} isn't a named view: they're {}",
                        crate::document::json::shown(view),
                        names.join(", ")
                    ),
                    "give one of the named views",
                ));
            }
        },
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
        errors.push(invalid(
            "highlight",
            format!(
                "{} elements highlighted, more than the {MAX_HIGHLIGHTS} a render highlights",
                request.highlight.len()
            ),
            &format!("highlight at most {MAX_HIGHLIGHTS} elements"),
        ));
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

/// Each name in `highlight` found in the first body whose element map has it, as a face or
/// an edge: one that isn't a name, or names nothing, `reference_broken` with the likeliest
/// candidates; a vertex or a solid `invalid_input`; each naming `highlight.N`. An unstable
/// name is taken, as a read takes one (M4.1 section 2). Every one's error is found, the
/// first [`PROBLEMS_LISTED`] listed.
fn highlighted(model: &Model, bodies: &[Body], highlight: &[String]) -> Result<Vec<Lit>, Problems> {
    if highlight.is_empty() {
        return Ok(Vec::new());
    }
    let maps: Vec<Option<&ElementMap>> = bodies.iter().map(|b| model.element_map(b.id)).collect();
    let document = model.document();
    let makers = |id: &FeatureId| {
        document
            .features
            .iter()
            .find(|f| &f.id == id)
            .map(|f| Maker::of(&f.operation))
    };
    let mut lit = Vec::new();
    let mut errors = Vec::new();
    let mut total = 0u64;
    for (i, text) in highlight.iter().enumerate() {
        let parameter = format!("highlight.{i}");
        let shown = crate::document::json::shown(text);
        let error = match Name::parse(text, &makers) {
            Err(why) => Error::new(
                ErrorCode::ReferenceBroken,
                format!("{parameter}, {shown}, isn't a name: {why}"),
            )
            .with_suggestion("name the element as Silico names it (feature.type.Face1, ...)"),
            Ok(name) => {
                let found = maps.iter().enumerate().find_map(|(body, map)| {
                    map.and_then(|map| map.element(&name)).map(|e| (body, e))
                });
                match found {
                    Some((body, element))
                        if matches!(element.kind, ElementKind::Face | ElementKind::Edge) =>
                    {
                        lit.push(Lit {
                            body,
                            kind: element.kind,
                            index: element.index.saturating_sub(1),
                        });
                        continue;
                    }
                    Some((_, element)) => Error::new(
                        ErrorCode::InvalidInput,
                        format!(
                            "{parameter}, {shown}, names {}: only a face or an edge has pixels \
                             of its own to highlight",
                            a_kind(element.kind)
                        ),
                    )
                    .with_suggestion("highlight the faces or edges around it"),
                    None => {
                        let known: Vec<&ElementMap> = maps.iter().flatten().copied().collect();
                        Error::new(
                            ErrorCode::ReferenceBroken,
                            format!("{parameter}, {shown}, names no element of the model's bodies"),
                        )
                        .with_candidates(crate::references::likeliest(&known, &name, name.kind()))
                        .with_suggestion(
                            "name the element as its body names it now: the candidates are the \
                             likeliest, none taken for it",
                        )
                    }
                }
            }
        };
        total += 1;
        if errors.len() < PROBLEMS_LISTED {
            errors.push(error.with_parameter(parameter));
        }
    }
    if errors.is_empty() {
        lit.sort();
        lit.dedup();
        Ok(lit)
    } else {
        Err(Problems::with_unlisted(
            errors.clone(),
            total - errors.len() as u64,
        ))
    }
}

/// Refuses bodies in view with more faces or edges than a render draws, counted from their
/// B-reps before any is meshed.
fn within_caps(kept: &[&Body]) -> Result<(), Problems> {
    let count = |of: fn(&silico_protocol::ShapeSummary) -> u32| {
        kept.iter()
            .filter_map(|b| b.built.summary())
            .map(|s| u64::from(of(s)))
            .sum::<u64>()
    };
    for (what, count, most) in [
        ("faces", count(|s| s.faces), MAX_RENDER_FACES),
        ("edges", count(|s| s.edges), MAX_RENDER_EDGES),
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
    let asked: Vec<(&Body, Pending<bool>)> = bodies
        .iter()
        .filter(|b| {
            b.bounds
                .is_some_and(|[low, high]| (0..3).all(|k| low[k] <= eye[k] && eye[k] <= high[k]))
        })
        .map(|b| {
            let request = KernelRequest::classify(&b.built.shape, eye);
            (b, on.kernel.submit(on.generation, request))
        })
        .collect();
    for (body, pending) in asked {
        if done(pending.wait())? {
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
    let Some(level) = half_pixel(camera, height).and_then(mesh_level) else {
        return Err(Problems::from(
            Error::new(
                ErrorCode::Internal,
                "the camera checked has no size of pixel to mesh to",
            )
            .with_suggestion(SILICO_BUG),
        ));
    };
    enum Got {
        Kept(std::sync::Arc<Mesh>),
        Asked(f64, Pending<MeshAt>),
    }
    let got: Vec<Got> = kept
        .iter()
        .map(|b| {
            let deflection = on.meshes.deflection_for(&b.key, level);
            match on.meshes.get(&b.key, deflection) {
                Some(mesh) => Got::Kept(mesh),
                None => Got::Asked(
                    deflection,
                    on.kernel.submit(
                        on.generation,
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
            Got::Asked(asked, pending) => Ok(on.meshes.put(&b.key, asked, done(pending.wait())?)),
        })
        .collect()
}

/// A kernel request's result: its error as the refusal, and one superseded (the read lane's
/// generation is never advanced, so it isn't) said so.
fn done<T>(outcome: Outcome<T>) -> Result<T, Problems> {
    match outcome {
        Outcome::Done(result) => result.map_err(Problems::from),
        Outcome::Superseded => Err(Problems::from(
            Error::new(
                ErrorCode::Internal,
                "the render's kernel request was superseded",
            )
            .with_suggestion("render again"),
        )),
    }
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
    let (parameter, suggestion) = match fit_side {
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
    };
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
    match camera.frame() {
        Ok(frame) => Camera {
            up: frame.up,
            ..camera.clone()
        },
        Err(_) => camera.clone(),
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
