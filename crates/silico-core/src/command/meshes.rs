//! A model's display meshes for a view (docs/design/m5.0-app.md, `meshes`, M5.5; pending
//! the maintainer, #196's decision 6(a)): each body at the largest power of two no coarser
//! than half a pixel, from the cache `render` meshes into, so the viewport and an agent's
//! render of one view mesh a body once and look the same (M4.6's R1). Every body asked is
//! meshed, never culled to the view, so a pan never needs a read. Each mesh face and edge
//! carries its element map's name and whether it's stable; each body its step's key, its
//! box, and the half pixels its level serves, so a client reads again only when its view
//! leaves them.
//!
//! The core's read, without its command: the session's read lane, the published reads'
//! limit, the response's 128 MiB bound, and the wire's form wait for the maintainer's M5.0
//! approval.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use silico_protocol::{Camera, Mesh};

use super::Problems;
use super::elements::built;
use super::render::{
    AnsweredCamera, Body, asked_view, bodies, camera_for, eye_outside, level_of, meshed,
};
use crate::document::FeatureId;
use crate::kernel::{ElementKind, ElementRef, Kernel};
use crate::meshes::{MeshCache, mesh_level};
use crate::naming::{ElementMap, Name};
use crate::recompute::{FeatureOutcome, Key, Model};
use crate::views::NamedView;
use crate::worker::SILICO_BUG;
use crate::{Error, ErrorCode};

/// A display mesh read asked for (the design's request, its session and model aside).
#[derive(Debug, Clone, PartialEq)]
pub struct Meshes {
    /// A named view, or none with a camera.
    pub view: Option<NamedView>,
    /// A camera, or none with a named view.
    pub camera: Option<Camera>,
    /// The view's size in pixels, each side from 1 to `MAX_RENDER_SIDE`, as `render`'s.
    pub width: u32,
    pub height: u32,
    /// The features whose solids to mesh, or none for every body.
    pub features: Option<Vec<FeatureId>>,
    /// The meshes the client holds already, whose geometry isn't given again.
    pub held: Vec<Held>,
}

/// A mesh a client holds: its body's step key and the deflection it was made at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub key: Key,
    pub deflection: f64,
}

/// What the read answers: the camera, as `render` answers it, and each solid meshed, in
/// timeline order.
#[derive(Debug, Clone)]
pub struct MeshesRead {
    pub camera: AnsweredCamera,
    pub bodies: Vec<MeshedBody>,
}

/// A solid's display mesh, and what it's of.
#[derive(Debug, Clone)]
pub struct MeshedBody {
    pub feature: FeatureId,
    /// Its number among the model's bodies, from 1, as the render's ID buffer writes it;
    /// none for a feature's solid another feature consumes.
    pub body: Option<u32>,
    /// Its step's key.
    pub key: Key,
    /// The deflection its view asked for (`mesh_level` of its half pixel).
    pub level: f64,
    /// The deflection the kernel used: the level, or the body's floor, which is coarser.
    pub deflection: f64,
    /// Its tight box, if it was measured.
    pub bounds: Option<[[f64; 3]; 2]>,
    /// The half pixels `[at_least, below)` this mesh serves: `[level, 2 · level)`; for a body
    /// meshed at its floor (the kernel used more than was asked), every half pixel whose
    /// level is under the floor, `[0, 2 · mesh_level(floor))`.
    pub serves: [f64; 2],
    /// Whether the client holds this mesh (its key at its deflection).
    pub held: bool,
    /// The kernel's mesh, shared with the cache; none if the client holds it.
    pub mesh: Option<Arc<Mesh>>,
    /// Each face, in the mesh's face order: its name, its stability, and its triangles.
    pub faces: Vec<MeshElement>,
    /// Each edge, in the mesh's edge order: its name, its stability, and its points.
    pub edges: Vec<MeshElement>,
}

/// A mesh face or edge: its element map's own name for it, whether that name is stable,
/// and how many of the mesh's triangles (a face) or edge points (an edge) are its, given
/// with the geometry (none for a mesh the client holds, which has them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshElement {
    pub name: Arc<Name>,
    pub stable: bool,
    pub count: Option<u32>,
}

/// What a mesh read runs on: the read's kernel client and the meshes kept beside the
/// kernel, which `render` meshes into.
pub struct MeshesOn<'a, K: Kernel> {
    pub kernel: &'a K,
    pub meshes: &'a MeshCache<Key>,
}

/// Meshes `model`'s solids for the view `request` asks, on `on`. Refused, each reason named
/// by its field: the view, the camera, and the size, as `render` refuses them (and a
/// perspective eye inside a solid, `camera.eye`); a feature the document hasn't, a sketch, or
/// one named twice (`features.N`); a feature that didn't build (its failure, as `check`
/// says it); and a kernel fault meshing one (naming the feature).
pub fn meshes<K: Kernel>(
    model: &Model,
    request: &Meshes,
    on: &MeshesOn<'_, K>,
) -> Result<MeshesRead, Problems> {
    let _span =
        tracing::info_span!("meshes", width = request.width, height = request.height).entered();
    let mut view_errors = Vec::new();
    let asked = asked_view(
        &request.view,
        &request.camera,
        request.width,
        request.height,
        &mut view_errors,
    );
    let all = bodies(model);
    let mut refused = Problems::none();
    for error in view_errors {
        refused.push(error);
    }
    let chosen = match &request.features {
        None => all.iter().map(|b| (b.clone(), Some(b.number))).collect(),
        Some(features) => chosen(model, &all, features, &mut refused),
    };
    let asked = match asked {
        Some(asked) if refused.is_empty() => asked,
        _ => return Err(refused),
    };
    // A named view fits every body, as render's does; a camera is taken as given.
    let (camera, answered, _) = camera_for(asked, &all, request.width, request.height)?;
    let solids: Vec<Body> = chosen.iter().map(|(solid, _)| solid.clone()).collect();
    // An eye inside any body is refused, as render refuses it, asked for or not.
    eye_outside(&camera, &all, on.kernel, session_closed)?;
    let held: BTreeSet<(Key, u64)> = request
        .held
        .iter()
        .map(|h| (h.key, h.deflection.to_bits()))
        .collect();
    // A body the client holds at the deflection it would be meshed at is answered without
    // the kernel, whether or not the shared cache still keeps its mesh: a held view of an
    // unchanged model never meshes again, but at a floor that's a power of two, which the
    // read can't tell from a level once the cache has dropped it.
    // A held deflection that isn't a power of two can only be its step's floor (levels are
    // powers of two, and a floor is its key's), so a body held at it, at a level under it,
    // is held though the cache has dropped its floor. A floor is 1e-4 of the kernel's box's
    // diagonal, a box about the tight one, so it's within [the tight box's floor, twice
    // it): a held deflection outside that, or not finite, isn't one.
    let held_floor = |solid: &Body, level: f64| {
        let [low, high] = solid.bounds?;
        let diagonal = libm::sqrt(
            (0..3)
                .map(|k| (high[k] - low[k]) * (high[k] - low[k]))
                .sum::<f64>(),
        );
        let least = silico_protocol::mesh_floor(diagonal);
        request.held.iter().find_map(|h| {
            let floor = h.deflection;
            (h.key == solid.key
                && floor.is_finite()
                && floor >= level
                && floor >= least
                && floor < 2.0 * least
                && mesh_level(floor) != Some(floor))
            .then_some(floor)
        })
    };
    let planned: Vec<(f64, f64)> = solids
        .iter()
        .map(|solid| {
            let level = level_of(&camera, request.height, solid);
            let deflection = on.meshes.deflection_for(&solid.key, level);
            match held_floor(solid, level) {
                Some(floor) if deflection == level => (level, floor),
                _ => (level, deflection),
            }
        })
        .collect();
    let to_mesh: Vec<&Body> = solids
        .iter()
        .zip(&planned)
        .filter(|(solid, (_, deflection))| !held.contains(&(solid.key, deflection.to_bits())))
        .map(|(solid, _)| solid)
        .collect();
    let mut made = meshed(
        &camera,
        request.height,
        &to_mesh,
        on.kernel,
        on.meshes,
        session_closed,
    )?
    .into_iter();
    let bodies = solids
        .iter()
        .zip(&chosen)
        .zip(planned)
        .map(|((solid, (_, number)), (level, deflection))| {
            let map = model
                .element_map(solid.id)
                .ok_or_else(|| unnamed(solid.id))?;
            let (level, deflection, mesh) = if held.contains(&(solid.key, deflection.to_bits())) {
                (level, deflection, None)
            } else {
                let Some(made) = made.next() else {
                    return Err(Problems::from(
                        Error::new(
                            ErrorCode::Internal,
                            format!("{} was asked to be meshed, and no mesh came", solid.id),
                        )
                        .with_feature(solid.id.as_str())
                        .with_suggestion(SILICO_BUG),
                    ));
                };
                consistent(solid, map, &made.mesh, on.meshes, made.deflection)?;
                (made.level, made.deflection, Some(made.mesh))
            };
            // Held at the deflection the kernel used (a floor it didn't know before): the
            // client has it too.
            let is_held = held.contains(&(solid.key, deflection.to_bits()));
            let mesh = mesh.filter(|_| !is_held);
            // Each element's count comes with the geometry only, so a held body's answer is
            // the same whether its mesh was kept or made again.
            let faces = named(
                map,
                ElementKind::Face,
                mesh.as_deref().map(|m| m.face_triangles.as_slice()),
                solid.id,
            )?;
            let edges = named(
                map,
                ElementKind::Edge,
                mesh.as_deref().map(|m| m.edge_point_counts.as_slice()),
                solid.id,
            )?;
            // At its floor when the kernel used more than was asked: a function of the read
            // and the solid alone, never of what the shared cache has seen.
            let serves = match (deflection > level, mesh_level(deflection)) {
                (true, Some(floor_level)) => [0.0, 2.0 * floor_level],
                _ => [level, 2.0 * level],
            };
            Ok(MeshedBody {
                feature: solid.id.clone(),
                body: *number,
                key: solid.key,
                level,
                deflection,
                bounds: solid.bounds,
                serves,
                held: is_held,
                mesh,
                faces,
                edges,
            })
        })
        .collect::<Result<Vec<_>, Problems>>()?;
    Ok(MeshesRead {
        camera: answered,
        bodies,
    })
}

/// The solids of `features`, in timeline order: each a body of the model or a feature's
/// solid another consumes. Every reason one can't be meshed goes in `refused`, each naming
/// `features.N` (the first hundred listed, all counted): a feature the document hasn't, a
/// sketch, one named twice, or one that didn't build (its failure).
fn chosen<'m>(
    model: &'m Model,
    all: &[Body<'m>],
    features: &[FeatureId],
    refused: &mut Problems,
) -> Vec<(Body<'m>, Option<u32>)> {
    let places: BTreeMap<&FeatureId, usize> = model
        .features()
        .iter()
        .enumerate()
        .map(|(at, (id, _))| (id, at))
        .collect();
    let bodies: BTreeMap<&FeatureId, &Body<'m>> = all.iter().map(|b| (b.id, b)).collect();
    let mut seen: BTreeSet<&FeatureId> = BTreeSet::new();
    let mut found: Vec<(usize, (Body<'m>, Option<u32>))> = Vec::new();
    for (place, id) in features.iter().enumerate() {
        let invalid = |message: String, suggestion: &str| {
            Error::new(ErrorCode::InvalidInput, message)
                .with_parameter(format!("features.{place}"))
                .with_suggestion(suggestion)
        };
        if !seen.insert(id) {
            refused.push_with(|| {
                invalid(
                    format!("feature {id} is named twice"),
                    "name each feature once",
                )
            });
            continue;
        }
        let Some(&at) = places.get(id) else {
            refused.push_with(|| {
                invalid(
                    format!("the document has no feature {id}"),
                    "name a feature of the document, by its ID",
                )
            });
            continue;
        };
        let (feature, outcome) = &model.features()[at];
        if let FeatureOutcome::Sketch(_) = outcome {
            refused.push_with(|| {
                invalid(
                    format!("{id} is a sketch, which makes no solid to mesh"),
                    "name a feature that makes a solid",
                )
                .with_feature(id.as_str())
            });
            continue;
        }
        if let Some(body) = bodies.get(id) {
            found.push((at, ((*body).clone(), Some(body.number))));
            continue;
        }
        match (built(model, id, false), outcome) {
            (Ok(built), FeatureOutcome::Built { key, .. }) => found.push((
                at,
                (
                    Body {
                        number: 0,
                        id: feature,
                        key: *key,
                        built,
                        bounds: built.summary().and_then(|s| s.bounds),
                    },
                    None,
                ),
            )),
            (Err(failed), _) => refused.absorb(failed),
            (Ok(_), _) => refused.push(
                Error::new(
                    ErrorCode::Internal,
                    format!("{id} built, but the model holds no step for it"),
                )
                .with_feature(id.as_str())
                .with_suggestion(SILICO_BUG),
            ),
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, solid)| solid).collect()
}

/// Each element of `kind` the mesh has (`counts`, one per element, in its order) by its
/// element map's own name, its stability, and its count. Refused as the core's bug if the
/// map names fewer: a map and its mesh are of one shape.
fn named(
    map: &ElementMap,
    kind: ElementKind,
    counts: Option<&[u32]>,
    feature: &FeatureId,
) -> Result<Vec<MeshElement>, Problems> {
    let total = map.count(kind);
    // `consistent` has checked the mesh against the solid already: a map that names another
    // number than its own solid holds is the core's.
    if let Some(counts) = counts
        && counts.len() != total
    {
        return Err(Problems::from(
            Error::new(
                ErrorCode::Internal,
                format!(
                    "{feature}'s element map names {total} {kind:?}s, and its mesh, checked, {}",
                    counts.len()
                ),
            )
            .with_feature(feature.as_str())
            .with_suggestion(SILICO_BUG),
        ));
    }
    // The map numbers elements from 1, in the mesh's (OCCT's) order.
    (1u32..)
        .take(total)
        .enumerate()
        .map(|(place, index)| {
            let Some(name) = map.name(ElementRef { kind, index }) else {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::Internal,
                        format!("{feature}'s element map has no name for its {kind:?} {index}"),
                    )
                    .with_feature(feature.as_str())
                    .with_suggestion(SILICO_BUG),
                ));
            };
            Ok(MeshElement {
                stable: !map.is_unstable(&name),
                name,
                count: counts.map(|counts| counts[place]),
            })
        })
        .collect()
}

/// Refuses `mesh` if its faces or edges don't number `solid`'s, as the worker's: by its
/// summary, or, for a solid never measured, its element map. It's dropped from the shared
/// cache (render may have kept it) either way, so the next read asks again.
fn consistent(
    solid: &Body,
    map: &ElementMap,
    mesh: &Mesh,
    cache: &MeshCache<Key>,
    deflection: f64,
) -> Result<(), Problems> {
    let (want_faces, want_edges) = match solid.built.summary() {
        Some(summary) => (summary.faces as usize, summary.edges as usize),
        None => (map.count(ElementKind::Face), map.count(ElementKind::Edge)),
    };
    let (faces, edges) = (mesh.face_triangles.len(), mesh.edge_point_counts.len());
    if (faces, edges) == (want_faces, want_edges) {
        return Ok(());
    }
    cache.forget(&solid.key, deflection);
    Err(inconsistent(
        solid.id,
        format!(
            "a mesh of {faces} faces and {edges} edges, for a solid of {want_faces} and {want_edges}"
        ),
    ))
}

/// A worker's mesh that isn't its solid's, as the kernel client refuses an inconsistent one.
fn inconsistent(feature: &FeatureId, cause: String) -> Problems {
    Problems::from(
        Error::new(
            ErrorCode::ProtocolError,
            "the kernel worker sent an inconsistent mesh",
        )
        .with_feature(feature.as_str())
        .with_cause(cause)
        .with_suggestion("start a new kernel worker"),
    )
}

/// The answer of a read stopped by its session closing.
fn session_closed() -> Error {
    super::session_closed("the read of its meshes finished", "read its meshes")
}

/// A built solid with no element map: the core's bug, since every built solid is named.
fn unnamed(feature: &FeatureId) -> Problems {
    Problems::from(
        Error::new(
            ErrorCode::Internal,
            format!("{feature}'s solid built, but its elements weren't named"),
        )
        .with_feature(feature.as_str())
        .with_suggestion(SILICO_BUG),
    )
}
