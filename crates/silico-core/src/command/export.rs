//! `export` (docs/design/m4.1-command-api.md, `export`; docs/ports/mesh-export.md,
//! docs/ports/step-export.md): the current model's bodies, or the features asked, as a
//! printable 3MF file or a STEP file. The request is checked in M4.1's order (the format,
//! the tolerance, the path, the features), every reason found refused at once. For 3MF,
//! each feature's solid is then meshed at the tolerance on the read's own client of the
//! kernel (as many at once as the machine runs), joined and checked as a slicer needs
//! ([`crate::export::printable`]), and written as one 3MF archive
//! ([`crate::export::three_mf`]); for STEP, every solid is written by the kernel as one
//! file, each a product named by its feature, its header written from the document. Once
//! the read settles, the file is saved through the writer every command's files share
//! (decision 35). Nothing is written for a request refused.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use super::{Problems, Running, SessionCommand, SessionId, Sessions};
use crate::document::{FeatureId, Length, Measure};
use crate::export::{
    Object, PrintableMesh, UNHOLDABLE_ADVICE, face_names, holds_in_3mf, printable, three_mf,
    unholdable, within_triangles,
};
use crate::file::{self, Output};
use crate::kernel::{Clients, Kernel, KernelRequest};
use crate::pool::{Outcome, Pending};
use crate::recompute::Model;
use crate::{Error, ErrorCode};

/// The formats `export` writes, by the words a request gives them, each with what it takes:
/// a mesh format its tolerance, an exact one nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExportFormat {
    /// 3MF (3MF Core 1.4.0): each body a mesh, as a slicer prints it, meshed at
    /// `tolerance`: the mesher's linear deflection, as FreeCAD's export takes it.
    ThreeMf { tolerance: Length },
    /// STEP (ISO 10303-21, AP214): each body's exact B-rep, as other CAD systems read it.
    Step,
}

impl ExportFormat {
    /// The format's word, as a request gives it, and its file's extension.
    pub const fn name(self) -> &'static str {
        match self {
            ExportFormat::ThreeMf { .. } => "3mf",
            ExportFormat::Step => "step",
        }
    }

    /// The mesh's tolerance, for a mesh format; none for an exact one.
    pub const fn tolerance(self) -> Option<Length> {
        match self {
            ExportFormat::ThreeMf { tolerance } => Some(tolerance),
            ExportFormat::Step => None,
        }
    }
}

/// The tolerance a 3MF export is meshed at when a client gives none (a request always gives
/// one; `silico export` without `--tolerance`): FreeCAD's 0.1 mm
/// ([`silico_protocol::DEFAULT_EXPORT_TOLERANCE`]), as a length.
pub const DEFAULT_TOLERANCE: Length = Length::from_nm(100_000);

/// An export asked for (M4.1's request, its session aside: an export reads the current
/// model only).
#[derive(Debug, Clone, PartialEq)]
pub struct Export {
    pub format: ExportFormat,
    /// Where to write the file: in the workspace, ending in the format's extension.
    pub path: PathBuf,
    /// Whether to replace a file already at `path`.
    pub replace: bool,
    /// The features whose solids to write, each by its ID, in the file's order; or none,
    /// for every body, in the timeline's order.
    pub features: Option<Vec<FeatureId>>,
}

/// What an export answers (M4.1's result): where the file was written, as the request gave
/// it (as `open` and `commit` answer theirs), its size, and each object in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub path: PathBuf,
    pub bytes: u64,
    pub objects: Vec<ExportedObject>,
}

/// One object of an export: its feature and name, its mesh, for a mesh format, and the
/// faces left out of the file as bounding no material, by their names.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportedObject {
    pub feature: FeatureId,
    pub name: String,
    /// The mesh written, for a mesh format (3MF); none for an exact one (STEP).
    pub mesh: Option<MeshWritten>,
    pub left_out: Vec<String>,
}

/// An object's mesh, as written: its triangles and vertices, and its volume, computed in
/// doubles from the coordinates as written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshWritten {
    pub triangles: u64,
    pub vertices: u64,
    pub volume: Measure,
}

impl<P: Clients + 'static> Sessions<P> {
    /// `export` (M4.8): the current model's bodies (or the features `request` names) as a
    /// file, as [`export`] writes one: meshed on the read's own client of the kernel, the
    /// file written once the read settles, so a cancelled export leaves no trace.
    pub fn export(&self, session: &SessionId, request: Export) -> Running<Exported> {
        let rules = self.write_rules();
        let step = self.step_exporter();
        self.read_current(session, SessionCommand::Export, move |read| {
            export(
                read.model(),
                &request,
                &ExportOn {
                    kernel: read.kernel(),
                    step: &*step,
                    document: read.path(),
                    workspace: rules.workspace.as_ref(),
                    held: &rules.held,
                    settle: &|| read.settle(),
                },
            )
        })
    }
}

/// What an export runs on: a kernel client (the session's read lane), what a file written
/// must keep clear of (decision 35), and the settle that comes before it.
pub struct ExportOn<'a, K: Kernel> {
    pub kernel: &'a K,
    /// What writes a STEP file: the STEP worker (`step_worker::StepWorkers`), or a test's.
    pub step: &'a dyn crate::step_worker::StepExporter,
    /// The document's path, whose file name a STEP file's header gives, and whose stem
    /// names its assembly.
    pub document: &'a str,
    /// The workspace a file written must be inside, if the sessions have one: a relative
    /// path is relative to its root.
    pub workspace: Option<&'a file::Workspace>,
    /// The documents the sessions hold, which a file written is never.
    pub held: &'a [PathBuf],
    /// Called just before the file is written, the export's one trace: refused (the read
    /// was cancelled, decision 34), nothing is written and its error is the answer.
    pub settle: &'a dyn Fn() -> Result<(), Error>,
}

/// Exports `model` as `request` asks, on `on` (the module's steps). Refused, each reason
/// named by its field, for a request that can't be written: a tolerance out of range, a
/// path decision 35 refuses, an empty or repeated list of features, a feature that has no
/// solid (its own failure, as `check` gives it), a name a 3MF file can't hold, or every
/// body of a model whose features didn't all build; and for a solid that isn't printable
/// at the tolerance, naming its feature and the faces at fault, or meshes of more triangles
/// in all than a file holds, refused once they're past it.
pub fn export<K: Kernel>(
    model: &Model,
    request: &Export,
    on: &ExportOn<'_, K>,
) -> Result<Exported, Problems> {
    let _span = tracing::info_span!(
        "export",
        format = request.format.name(),
        tolerance = request.format.tolerance().map(Length::mm)
    )
    .entered();
    let output = Output {
        field: "path",
        extension: request.format.name(),
        writer: "an export",
        replace: request.replace,
        workspace: on.workspace,
        held: on.held,
    };
    let solids = checked(model, request, &output)?;
    let (bytes, objects) = match request.format {
        ExportFormat::ThreeMf { tolerance } => meshed(model, &solids, tolerance.mm(), on)?,
        ExportFormat::Step => stepped(model, &solids, on)?,
    };
    (on.settle)().map_err(Problems::from)?;
    file::write_output(&request.path, &bytes, &output).map_err(Problems::from)?;
    Ok(Exported {
        path: request.path.clone(),
        bytes: bytes.len() as u64,
        objects,
    })
}

/// `solids` as a 3MF archive, each meshed at `tolerance` mm, and each object as written.
fn meshed<K: Kernel>(
    model: &Model,
    solids: &[Solid<'_>],
    tolerance: f64,
    on: &ExportOn<'_, K>,
) -> Result<(Vec<u8>, Vec<ExportedObject>), Problems> {
    let ask = |solid: &Solid| {
        KernelRequest::export_mesh(&solid.built.shape, tolerance)
            .map(|request| on.kernel.submit(on.kernel.generation(), request))
            .map_err(|e| Problems::from(e.with_feature(solid.id.as_str())))
    };
    // A few solids asked of the kernel ahead, as many as the machine runs at once, so meshes
    // done but not yet taken are never more than that; each taken, checked, and counted in
    // turn, so a file past the triangles one may hold is refused once it's past them.
    let ahead = std::thread::available_parallelism().map_or(1, usize::from);
    let mut asked: VecDeque<Pending<_>> = VecDeque::with_capacity(ahead);
    let mut meshes = Vec::with_capacity(solids.len());
    let mut triangles = 0_u64;
    for (n, solid) in solids.iter().enumerate() {
        while asked.len() < ahead {
            let Some(next) = solids.get(n + asked.len()) else {
                break;
            };
            asked.push_back(ask(next)?);
        }
        let Some(pending) = asked.pop_front() else {
            return Err(Problems::from(
                Error::new(
                    ErrorCode::Internal,
                    format!("{}'s mesh was never asked of the kernel", solid.id),
                )
                .with_suggestion(crate::worker::SILICO_BUG),
            ));
        };
        // A mesh that took past a kernel request's time is the tolerance's to change (M4.1's
        // `export`).
        let at_feature = |e: Error| {
            let e = match e.code() {
                ErrorCode::WorkerTimeout => e.with_suggestion(TIMED_OUT),
                _ => e,
            };
            super::of_solid(solid.id, e)
        };
        let mesh = super::done(pending.wait(), session_closed).map_err(|p| p.map(at_feature))?;
        // Asked by `printable` only when a face was raised.
        let unraised = || {
            let request = KernelRequest::unraised_export_mesh(&solid.built.shape, tolerance)?;
            match on.kernel.submit(on.kernel.generation(), request).wait() {
                Outcome::Done(result) => result,
                Outcome::Superseded => Err(session_closed()),
            }
        };
        let printable = printable(&mesh, unraised, model.element_map(solid.id))
            .map_err(|e| Problems::from(at_feature(e)))?;
        triangles = triangles.saturating_add(printable.triangles().len() as u64);
        within_triangles(triangles).map_err(Problems::from)?;
        meshes.push(printable);
    }
    let objects: Vec<Object<'_>> = solids
        .iter()
        .zip(&meshes)
        .map(|(solid, mesh)| Object {
            name: solid.name,
            part_number: solid.id.as_str(),
            mesh,
        })
        .collect();
    let bytes = three_mf(&objects).map_err(Problems::from)?;
    let answered = solids
        .iter()
        .zip(&meshes)
        .map(|(solid, mesh)| object(solid, mesh))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((bytes, answered))
}

/// `solids` as one STEP file, written by the kernel (docs/ports/step-export.md): each a
/// product, its id the feature's ID and its name the feature's, under an assembly named by
/// the document's stem if there are two or more; the header's file name the document's;
/// every string encoded ([`silico_protocol::step_encoded`]), so every name is written as it
/// is. Each object's faces left out named by its feature's element map.
fn stepped<K: Kernel>(
    model: &Model,
    solids: &[Solid<'_>],
    on: &ExportOn<'_, K>,
) -> Result<(Vec<u8>, Vec<ExportedObject>), Problems> {
    let document = Path::new(on.document);
    let text = |component: Option<&std::ffi::OsStr>| {
        silico_protocol::step_encoded(
            &component.map_or_else(String::new, |c| c.to_string_lossy().into_owned()),
        )
    };
    let export = silico_protocol::ExportStep {
        parts: solids
            .iter()
            .map(|solid| silico_protocol::StepPart {
                shape: solid.built.shape.clone(),
                id: silico_protocol::step_encoded(solid.id.as_str()),
                name: silico_protocol::step_encoded(solid.name),
            })
            .collect(),
        assembly: text(document.file_stem()),
        file_name: text(document.file_name()),
    };
    let (file, writer) = on.step.export_step(export).map_err(|e| {
        // A file that took past a request's time is the bodies' to split.
        let e = match e.code() {
            ErrorCode::WorkerTimeout => e.with_suggestion(STEP_TIMED_OUT),
            _ => e,
        };
        Problems::from(match solids {
            // One part: whatever the writer refused is its.
            [solid] => super::of_solid(solid.id, e),
            _ => e,
        })
    })?;
    // Its bytes are its writer's: which build wrote it, as a kernel's identity keys its results.
    tracing::info!(writer = %writer, bytes = file.step.as_bytes().len(), "STEP file written");
    if let Some(refused) = unwritten(model, solids, &file) {
        return Err(refused);
    }
    let objects = solids
        .iter()
        .zip(&file.left_out)
        .map(|(solid, faces)| {
            let left_out = face_names(model.element_map(solid.id), faces)
                .map_err(|e| super::of_solid(solid.id, e))?;
            Ok(ExportedObject {
                feature: solid.id.clone(),
                name: solid.name.to_owned(),
                mesh: None,
                left_out,
            })
        })
        .collect::<Result<Vec<_>, Error>>()
        .map_err(Problems::from)?;
    Ok((file.step.into_bytes(), objects))
}

/// What a STEP export that took past a request's time suggests.
const STEP_TIMED_OUT: &str =
    "export fewer features at once: the STEP writer writes every body in one request";

/// What an export whose mesh took past a kernel request's time suggests.
const TIMED_OUT: &str =
    "export with a coarser tolerance: a finer one makes a larger mesh, which takes longer";

/// A solid to export: its feature, its name, and its build.
struct Solid<'m> {
    id: &'m FeatureId,
    name: &'m str,
    built: &'m crate::recompute::Built,
}

/// The solids `request` asks for, or every reason it's refused, in M4.1's order: the
/// tolerance, the path (as it would be written, nothing written yet), the features (none
/// listed; each listed, in the list's order, if it's listed already, the document hasn't
/// it, it has no solid, or its name is one a 3MF file can't hold), and, for every body, a
/// model whose features didn't all build, whose bodies aren't the ones its document
/// describes: each failure, as `check` gives it.
fn checked<'m>(
    model: &'m Model,
    request: &Export,
    output: &Output,
) -> Result<Vec<Solid<'m>>, Problems> {
    let mut refused = Problems::none();
    if let Some(tolerance) = request.format.tolerance()
        && silico_protocol::check_export_tolerance(tolerance.mm()).is_err()
    {
        // In the document's own numbers, as the request wrote it.
        let bound = |mm: f64| Length::rounded(mm).map_or_else(|| mm.to_string(), |l| l.to_string());
        refused.push(
            Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "tolerance must be a length from {} mm to {} mm, got {}",
                    bound(silico_protocol::MIN_EXPORT_TOLERANCE),
                    bound(silico_protocol::MAX_EXPORT_TOLERANCE),
                    tolerance
                ),
            )
            .with_parameter("tolerance")
            .with_suggestion("give a tolerance in range; 0.1 mm is FreeCAD's default"),
        );
    }
    if let Err(e) = file::check_output(&request.path, output) {
        refused.push(e);
    }
    // Each feature of the document by its ID, its name with it, found once each.
    let features: BTreeMap<&FeatureId, &str> = model
        .document()
        .features
        .iter()
        .map(|f| (&f.id, f.name.as_str()))
        .collect();
    let mut solids = Vec::new();
    // A kernel fault stops every feature that uses what it stopped with the same error: each
    // fault said once, for the first feature it stopped.
    let mut faults_said: BTreeSet<(ErrorCode, String)> = BTreeSet::new();
    let mut solid = |id: &'m FeatureId, parameter: Option<String>, refused: &mut Problems| {
        let built = match super::elements::built(model, id, parameter.is_some()) {
            Ok(built) => built,
            Err(problems) => {
                let unlisted = problems.total() - problems.errors().len() as u64;
                for e in problems.errors() {
                    if e.code().is_kernel_fault()
                        && !faults_said.insert((e.code(), e.message().to_owned()))
                    {
                        continue;
                    }
                    // Named by its place in the request's list, not as `feature`.
                    refused.push_with(|| match (&parameter, e.parameter()) {
                        (Some(at), Some("feature")) => e.clone().with_parameter(at.clone()),
                        _ => e.clone(),
                    });
                }
                refused.absorb(Problems::with_unlisted(Vec::new(), unlisted));
                return;
            }
        };
        let name = features.get(id).copied().unwrap_or(id.as_str());
        // STEP encodes every character; XML 1.0 holds most.
        if matches!(request.format, ExportFormat::ThreeMf { .. }) && !holds_in_3mf(name) {
            refused.push_with(|| {
                Error::new(
                    ErrorCode::InvalidInput,
                    unholdable(&format!("name of {id}"), name),
                )
                .with_feature(id.as_str())
                .with_parameter("name")
                .with_suggestion(UNHOLDABLE_ADVICE)
            });
            return;
        }
        solids.push(Solid { id, name, built });
    };
    match &request.features {
        None => {
            if model.errors().next().is_some() {
                every_body_failed(model, &mut refused);
            } else {
                for id in model.bodies() {
                    solid(id, None, &mut refused);
                }
            }
        }
        Some(listed) => {
            if listed.is_empty() {
                refused.push(
                    Error::new(
                        ErrorCode::InvalidInput,
                        "features must name at least one feature, or be null for every body",
                    )
                    .with_parameter("features")
                    .with_suggestion("give the features to export, or null for every body"),
                );
            }
            let mut seen = BTreeSet::new();
            for (n, id) in listed.iter().enumerate() {
                if !seen.insert(id) {
                    refused.push_with(|| {
                        Error::new(
                            ErrorCode::InvalidInput,
                            format!("features.{n}, {id}, is listed already"),
                        )
                        .with_feature(id.as_str())
                        .with_parameter(format!("features.{n}"))
                        .with_suggestion("list each feature once")
                    });
                    continue;
                }
                // The model's own ID, which lives as long as it does.
                match features.get_key_value(id) {
                    Some((&id, _)) => solid(id, Some(format!("features.{n}")), &mut refused),
                    None => refused.push_with(|| {
                        Error::new(
                            ErrorCode::InvalidInput,
                            format!("the document has no feature {id}"),
                        )
                        .with_parameter(format!("features.{n}"))
                        .with_suggestion("name a feature of the document, by its ID")
                    }),
                }
            }
        }
    }
    if refused.total() == 0 && solids.is_empty() {
        // As FreeCAD refuses an export with nothing to mesh (04005b5e45).
        refused.push(
            Error::new(ErrorCode::InvalidInput, "the model has no bodies to export")
                .with_suggestion("add a feature that makes a solid, then export"),
        );
    }
    refused.into_result()?;
    Ok(solids)
}

/// Why every body of `model`, some of whose features didn't build, isn't exported: its
/// bodies aren't the document's (a failed feature consumes nothing, so what it would have
/// changed is a body as it was), and SolidWorks warns before it saves a part with rebuild
/// errors as a mesh. Said first, then the kernel fault that stopped a feature, if one did,
/// once (as `check` sets it aside), then each failure of the document's as `check` gives it.
/// If a fault alone stopped them, refused with it alone, as a read of a solid it stopped is
/// (`elements::built`): nothing of the document's needs mending.
fn every_body_failed(model: &Model, refused: &mut Problems) {
    let fault = model.errors().find(|e| e.code().is_kernel_fault());
    let mut own = model
        .errors()
        .filter(|e| !e.code().is_kernel_fault())
        .peekable();
    if own.peek().is_none() {
        if let Some(fault) = fault {
            refused.push(super::elements::worded(fault));
        }
        return;
    }
    refused.push(
        Error::new(
            ErrorCode::InvalidInput,
            "every body was asked for, but some of the document's features didn't build, so \
             the bodies aren't the ones it describes",
        )
        .with_parameter("features")
        .with_suggestion(
            "mend each feature that didn't build (each is listed after this), or list the \
             features to export",
        ),
    );
    if let Some(fault) = fault {
        refused.push(super::elements::worded(fault));
    }
    for error in own {
        refused.push_with(|| error.clone());
    }
    refused.absorb(Problems::with_unlisted(Vec::new(), model.unlisted_errors()));
}

/// The object `solid`'s `mesh` is in the result.
fn object(solid: &Solid, mesh: &PrintableMesh) -> Result<ExportedObject, Problems> {
    let volume = Measure::rounded(mesh.volume()).ok_or_else(|| {
        Problems::from(
            Error::new(
                ErrorCode::Internal,
                format!("{}'s mesh has a volume a measure can't hold", solid.id),
            )
            .with_feature(solid.id.as_str())
            .with_suggestion(crate::worker::SILICO_BUG),
        )
    })?;
    Ok(ExportedObject {
        feature: solid.id.clone(),
        name: solid.name.to_owned(),
        mesh: Some(MeshWritten {
            triangles: mesh.triangles().len() as u64,
            vertices: mesh.vertices().len() as u64,
            volume,
        }),
        left_out: mesh.left_out().to_vec(),
    })
}

/// The refusal of a STEP file its writer didn't write whole (docs/ports/step-export.md,
/// *Edge cases*), never a file with geometry missing: what the writer's check list says it
/// couldn't transfer of a part (other than the faces left out), one error a part, naming its
/// feature and the elements at fault by its element map, OCCT's words its cause; and fewer
/// faces, or face bounds (a face's outer edge and its holes), in the file than the parts
/// have that bound material, which the writer drops with only a warning. One part's, each
/// names its feature. None if it's whole.
fn unwritten(
    model: &Model,
    solids: &[Solid<'_>],
    file: &silico_protocol::StepFile,
) -> Option<Problems> {
    let short = |count: silico_protocol::StepCount| count.written < count.expected;
    if file.unwritten.is_empty() && !short(file.faces) && !short(file.bounds) {
        return None;
    }
    // Of no part: of the one there is, if there's one.
    let of = |part: Option<u32>| match (part, solids) {
        (Some(p), _) => solids.get(p as usize),
        (None, [solid]) => Some(solid),
        (None, _) => None,
    };
    let advice = |solid: Option<&Solid<'_>>| match solid {
        Some(_) => {
            "this solid can't be exported as STEP until the kernel writes it whole: there's no \
             workaround in the request; export it as 3MF, or rebuild it otherwise"
        }
        None => "export the bodies one at a time, to find which the writer can't write whole",
    };
    let named = |error: Error, solid: Option<&Solid<'_>>| match solid {
        Some(solid) => error.with_feature(solid.id.as_str()),
        None => error,
    };
    let mut refused = Problems::none();
    let mut parts: BTreeMap<Option<u32>, Vec<&silico_protocol::StepUnwritten>> = BTreeMap::new();
    for entry in &file.unwritten {
        parts.entry(entry.part).or_default().push(entry);
    }
    for (part, entries) in parts {
        let solid = of(part);
        let elements: BTreeSet<silico_protocol::ElementRef> =
            entries.iter().filter_map(|e| e.element).collect();
        let map = solid.and_then(|s| model.element_map(s.id));
        let names: Vec<String> = elements
            .iter()
            .take(silico_protocol::FACES_NAMED)
            .map(|&element| {
                map.and_then(|m| m.name(element))
                    .map_or_else(|| element.to_string(), |n| n.to_string())
            })
            .collect();
        let messages: Vec<&str> = entries
            .iter()
            .flat_map(|e| e.messages.iter().map(String::as_str))
            .collect();
        let whose = solid.map_or_else(|| "the file".to_owned(), |s| format!("{}'s solid", s.id));
        let error = Error::new(
            ErrorCode::InvalidResult,
            format!(
                "the STEP writer couldn't write all of {whose}: OCCT reports what it couldn't \
                 transfer"
            ),
        )
        .with_elements(names)
        .with_elements_total(elements.len() as u64)
        .with_cause(if messages.is_empty() {
            "OCCT's check list names it, with no message".to_owned()
        } else {
            messages.join("; ")
        })
        .with_suggestion(advice(solid));
        refused.push(named(error, solid));
    }
    for (count, what) in [(file.faces, "faces"), (file.bounds, "face bounds")] {
        if short(count) {
            let solid = of(None);
            let error = Error::new(
                ErrorCode::InvalidResult,
                format!(
                    "the STEP file holds {} of the {} {what} the bodies have that bound \
                     material: OCCT's writer dropped {}",
                    count.written,
                    count.expected,
                    count.expected - count.written
                ),
            )
            .with_cause("OCCT's writer drops what it can't map with only a warning")
            .with_suggestion(advice(solid));
            refused.push(named(error, solid));
        }
    }
    Some(refused)
}

/// The answer of an export stopped by its session closing, as `render` answers one.
fn session_closed() -> Error {
    super::session_closed("the export finished", "export")
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_TOLERANCE;

    #[test]
    fn the_default_tolerance_is_freecads() {
        assert_eq!(
            DEFAULT_TOLERANCE.mm(),
            silico_protocol::DEFAULT_EXPORT_TOLERANCE
        );
    }
}
