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

use std::collections::BTreeSet;
use std::sync::Arc;

use silico_protocol::{Camera, Mesh};

use super::Problems;
use super::elements::built;
use super::render::{AnsweredCamera, Body, asked_view, bodies, camera_for, eye_outside, meshed};
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
    /// meshed at its floor, every half pixel whose level is at or under the floor,
    /// `[0, 2 · mesh_level(floor))`.
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
/// and how many of the mesh's triangles (a face) or edge points (an edge) are its.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshElement {
    pub name: Arc<Name>,
    pub stable: bool,
    pub count: u32,
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
    let mut errors = Vec::new();
    let asked = asked_view(
        &request.view,
        &request.camera,
        request.width,
        request.height,
        &mut errors,
    );
    let all = bodies(model);
    let chosen = match &request.features {
        None => Ok(all.iter().map(|b| (b.clone(), Some(b.number))).collect()),
        Some(features) => chosen(model, &all, features),
    };
    let chosen = match (asked, chosen) {
        (Some(asked), Ok(chosen)) if errors.is_empty() => (asked, chosen),
        (_, Err(refused)) => {
            errors.extend(refused);
            return Err(Problems::of(errors));
        }
        _ => return Err(Problems::of(errors)),
    };
    let (asked, chosen) = chosen;
    // A named view fits every body, as render's does; a camera is taken as given.
    let (camera, answered, _) = camera_for(asked, &all, request.width, request.height)?;
    let solids: Vec<Body> = chosen.iter().map(|(solid, _)| solid.clone()).collect();
    eye_outside(&camera, &solids, on.kernel)?;
    let kept: Vec<&Body> = solids.iter().collect();
    let made = meshed(&camera, request.height, &kept, on.kernel, on.meshes)?;
    let held: BTreeSet<(Key, u64)> = request
        .held
        .iter()
        .map(|h| (h.key, h.deflection.to_bits()))
        .collect();
    let bodies = solids
        .iter()
        .zip(&chosen)
        .zip(made)
        .map(|((solid, (_, number)), made)| {
            let map = model
                .element_map(solid.id)
                .ok_or_else(|| unnamed(solid.id))?;
            let faces = named(map, ElementKind::Face, &made.mesh.face_triangles, solid.id)?;
            let edges = named(
                map,
                ElementKind::Edge,
                &made.mesh.edge_point_counts,
                solid.id,
            )?;
            let at_floor = made.deflection > made.level
                || on.meshes.floor(&solid.key) == Some(made.deflection);
            let serves = match (at_floor, mesh_level(made.deflection)) {
                (true, Some(floor_level)) => [0.0, 2.0 * floor_level],
                _ => [made.level, 2.0 * made.level],
            };
            let is_held = held.contains(&(solid.key, made.deflection.to_bits()));
            Ok(MeshedBody {
                feature: solid.id.clone(),
                body: *number,
                key: solid.key,
                level: made.level,
                deflection: made.deflection,
                bounds: solid.bounds,
                serves,
                held: is_held,
                mesh: (!is_held).then_some(made.mesh),
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
/// solid another consumes. Every reason one can't be meshed is found, each naming
/// `features.N`: a feature the document hasn't, a sketch, one named twice, or one that
/// didn't build (its failure).
fn chosen<'m>(
    model: &'m Model,
    all: &[Body<'m>],
    features: &[FeatureId],
) -> Result<Vec<(Body<'m>, Option<u32>)>, Vec<Error>> {
    let mut errors = Vec::new();
    let mut found: Vec<(usize, (Body<'m>, Option<u32>))> = Vec::new();
    for (place, id) in features.iter().enumerate() {
        let parameter = format!("features.{place}");
        let invalid = |message: String, suggestion: &str| {
            Error::new(ErrorCode::InvalidInput, message)
                .with_parameter(parameter.clone())
                .with_suggestion(suggestion)
        };
        if features[..place].contains(id) {
            errors.push(invalid(
                format!("feature {id} is named twice"),
                "name each feature once",
            ));
            continue;
        }
        let Some(at) = model.features().iter().position(|(f, _)| f == id) else {
            errors.push(invalid(
                format!("the document has no feature {id}"),
                "name a feature of the document, by its ID",
            ));
            continue;
        };
        if let Some(FeatureOutcome::Sketch(_)) = model.outcome(id) {
            errors.push(
                invalid(
                    format!("{id} is a sketch, which makes no solid to mesh"),
                    "name a feature that makes a solid",
                )
                .with_feature(id.as_str()),
            );
            continue;
        }
        if let Some(body) = all.iter().find(|b| b.id == id) {
            found.push((at, (body.clone(), Some(body.number))));
            continue;
        }
        match (built(model, id, false), model.outcome(id)) {
            (Ok(built), Some(FeatureOutcome::Built { key, .. })) => found.push((
                at,
                (
                    Body {
                        number: 0,
                        id: &model.features()[at].0,
                        key: *key,
                        built,
                        bounds: built.summary().and_then(|s| s.bounds),
                    },
                    None,
                ),
            )),
            (Err(refused), _) => errors.extend(refused.errors().iter().cloned()),
            (Ok(_), _) => errors.push(
                Error::new(
                    ErrorCode::Internal,
                    format!("{id} built, but the model holds no step for it"),
                )
                .with_feature(id.as_str())
                .with_suggestion(SILICO_BUG),
            ),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    found.sort_by_key(|(at, _)| *at);
    Ok(found.into_iter().map(|(_, solid)| solid).collect())
}

/// Each element of `kind` the mesh has (`counts`, one per element, in its order) by its
/// element map's own name, its stability, and its count. Refused as the core's bug if the
/// map names fewer: a map and its mesh are of one shape.
fn named(
    map: &ElementMap,
    kind: ElementKind,
    counts: &[u32],
    feature: &FeatureId,
) -> Result<Vec<MeshElement>, Problems> {
    counts
        .iter()
        // The map numbers elements from 1, in the mesh's (OCCT's) order.
        .zip(1u32..)
        .map(|(&count, index)| {
            let Some(name) = map.name(ElementRef { kind, index }) else {
                return Err(Problems::from(
                    Error::new(
                        ErrorCode::Internal,
                        format!(
                            "{feature}'s element map names fewer {kind:?}s than its mesh holds"
                        ),
                    )
                    .with_feature(feature.as_str())
                    .with_suggestion(SILICO_BUG),
                ));
            };
            Ok(MeshElement {
                stable: !map.is_unstable(&name),
                name,
                count,
            })
        })
        .collect()
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
