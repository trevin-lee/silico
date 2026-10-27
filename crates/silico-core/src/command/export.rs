//! `export` (docs/design/m4.1-command-api.md, `export`; docs/ports/mesh-export.md,
//! docs/ports/step-export.md): the current model's bodies, or the features asked, as a
//! printable 3MF file or a STEP file. The request is checked in M4.1's order (the format,
//! the tolerance, the path, the features), every reason found refused at once. For 3MF,
//! each feature's solid is then meshed at the tolerance on the read's own client of the
//! kernel (one export mesh at a time across the sessions, [`MeshTurns`]), joined and checked
//! as a slicer needs
//! ([`crate::export::printable`]), and written as one 3MF archive
//! ([`crate::export::three_mf`]); for STEP, every solid is written by the STEP worker as one
//! file, each a product named by its feature, its header written from the document. Once
//! the read settles, the file is saved through the writer every command's files share
//! (decision 35). Nothing is written for a request refused.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};

use super::{Problems, Running, SessionCommand, SessionId, Sessions};
use crate::document::{ElementName, FeatureId, Length, Measure};
use crate::export::{
    Object, PrintableMesh, UNHOLDABLE_ADVICE, face_names, holds_in_3mf, printable, three_mf,
    unholdable, within_triangles,
};
use crate::file::{self, Output};
use crate::kernel::{Clients, Kernel, KernelRequest};
use crate::pool::Outcome;
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

/// The least and the most tolerance a 3MF export takes, 1e-5 and 1e8 mm
/// ([`silico_protocol::MIN_EXPORT_TOLERANCE`], [`silico_protocol::MAX_EXPORT_TOLERANCE`]), as
/// lengths: what `version` gives.
pub const TOLERANCES: [Length; 2] = [Length::from_nm(10), Length::from_nm(100_000_000_000_000)];

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
    pub left_out: Vec<ElementName>,
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
        let turns = self.export_turns();
        self.read_current(session, SessionCommand::Export, move |read| {
            // Its STEP writing stopped with the read, as its meshing is (the read's client),
            // and its wait for a turn to mesh.
            read.exporting_step(&*step, |step| {
                read.meshing(&turns, |stopped| {
                    export(
                        read.model(),
                        &request,
                        &ExportOn {
                            kernel: read.kernel(),
                            step,
                            document: read.path(),
                            workspace: rules.workspace.as_ref(),
                            held: &rules.held,
                            turns: &turns,
                            stopped,
                            settle: &|| read.settle(),
                        },
                    )
                })
            })
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
    /// Where each solid's mesh waits its turn: one export mesh at a time across the sessions.
    pub turns: &'a MeshTurns,
    /// Whether the export was stopped (cancelled, or its session closed): a wait for a turn
    /// ends then, [`MeshTurns::wake`] called as it's stopped.
    pub stopped: &'a dyn Fn() -> bool,
    /// Called just before the file is written, the export's one trace: refused (the read
    /// was cancelled, decision 34), nothing is written and its error is the answer.
    pub settle: &'a dyn Fn() -> Result<(), Error>,
}

/// The turn to mesh a solid for export, one at a time across every session of a process
/// (owned by [`Sessions`], one for all of them): a mesh at a fine tolerance takes OCCT tens of
/// seconds and gigabytes (about 50 s and 3 GB at 0.001 mm on curved faces), its only bound
/// the kernel request's timeout, so meshes on every core at once would take tens of
/// gigabytes before the first timed out. A turn is held while its mesh is asked, made, and
/// checked. Turns are given in the order they're asked for, as the kernel pool serves its
/// clients in turn: an export that gives one back and asks again for its next solid goes
/// behind the exports already waiting, so a long export can't keep another session's (and
/// that session's whole queue) waiting for its whole length. A wait for one ends when the
/// export is stopped, its place in the line given up.
#[derive(Debug, Default)]
pub struct MeshTurns {
    line: Mutex<Line>,
    /// A turn was given back, a waiting export left the line, or one was stopped.
    freed: Condvar,
}

/// Who holds the turn to mesh, and who waits for it, in order: the line's every decision,
/// made under [`MeshTurns`]'s lock, which waits and wakes as they say.
#[derive(Debug, Default)]
struct Line {
    /// Whether a turn is held.
    held: bool,
    /// The ticket the next export to wait takes.
    next: u64,
    /// The tickets of the exports waiting, the first served first.
    waiting: VecDeque<u64>,
}

impl Line {
    /// Takes the turn for the export holding `ticket` (none: one not in line), and says
    /// whether it did: only when no turn is held, and the export is first in line, or, not
    /// in line, nobody waits (a newcomer never goes ahead of the line). Its place given up
    /// once it has the turn.
    fn take(&mut self, ticket: Option<u64>) -> bool {
        let first = match ticket {
            Some(ticket) => self.waiting.front() == Some(&ticket),
            None => self.waiting.is_empty(),
        };
        if self.held || !first {
            return false;
        }
        if ticket.is_some() {
            self.waiting.pop_front();
        }
        self.held = true;
        true
    }

    /// A place at the back of the line: its ticket.
    fn join(&mut self) -> u64 {
        let ticket = self.next;
        self.next = ticket.wrapping_add(1);
        self.waiting.push_back(ticket);
        ticket
    }

    /// `ticket`'s place given up, and whether the exports waiting must be woken to look: when
    /// it was first and no turn is held, the one behind it may take the turn now. (Behind the
    /// first, or with a turn held, nobody may take one it couldn't before: the turn given back
    /// wakes them.)
    fn leave(&mut self, ticket: u64) -> bool {
        let first = self.waiting.front() == Some(&ticket);
        self.waiting.retain(|&waiting| waiting != ticket);
        first && !self.held && !self.waiting.is_empty()
    }

    /// The turn given back, and whether the exports waiting must be woken to look: whether
    /// any wait, the first of them free to take it now.
    fn give_back(&mut self) -> bool {
        self.held = false;
        !self.waiting.is_empty()
    }
}

/// A turn to mesh, held until it's dropped.
#[must_use = "dropping a turn gives it back"]
struct MeshTurn<'a>(&'a MeshTurns);

impl MeshTurns {
    pub fn new() -> Self {
        Self::default()
    }

    /// The turn to mesh `feature`'s solid, once no other export holds it and every export
    /// waiting before this one has had its own; none if `stopped` says the export was
    /// stopped, before or while it waits ([`wake`](Self::wake) wakes it to look).
    fn take(&self, feature: &FeatureId, stopped: &dyn Fn() -> bool) -> Option<MeshTurn<'_>> {
        let mut line = crate::worker::lock(&self.line);
        let mut ticket = None;
        loop {
            if stopped() {
                if let Some(ticket) = ticket {
                    self.leave(&mut line, ticket);
                }
                return None;
            }
            if line.take(ticket) {
                if ticket.is_some() {
                    tracing::info!(
                        feature = feature.as_str(),
                        "the turn to mesh for export, after another export's mesh"
                    );
                }
                return Some(MeshTurn(self));
            }
            if ticket.is_none() {
                ticket = Some(line.join());
                tracing::info!(
                    feature = feature.as_str(),
                    ahead = line.waiting.len().saturating_sub(1),
                    "waiting for another export's mesh: one export mesh at a time"
                );
            }
            line = self
                .freed
                .wait(line)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// `ticket`'s place in `line` (this one's, locked) given up, the exports waiting woken if
    /// the one behind it may take the turn now.
    fn leave(&self, line: &mut Line, ticket: u64) {
        if line.leave(ticket) {
            self.freed.notify_all();
        }
    }

    /// Wakes every export waiting for a turn to look whether it was stopped: called once a
    /// `stopped` given to a wait says so. Under the lock, so a waiter that looked before it
    /// waited is waiting when it's told.
    pub(crate) fn wake(&self) {
        let _line = crate::worker::lock(&self.line);
        self.freed.notify_all();
    }
}

impl Drop for MeshTurn<'_> {
    fn drop(&mut self) {
        if crate::worker::lock(&self.0.line).give_back() {
            self.0.freed.notify_all();
        }
    }
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
    let mut meshes = Vec::with_capacity(solids.len());
    let mut triangles = 0_u64;
    for solid in solids {
        let request = KernelRequest::export_mesh(&solid.built.shape, tolerance)
            .map_err(|e| Problems::from(e.with_feature(solid.id.as_str())))?;
        // One solid at a time, its turn held until it's checked (its unraised mesh, if one,
        // asked under it too): a mesh at a fine tolerance takes OCCT tens of seconds and
        // gigabytes, so meshes on every core at once, this export's or other sessions', would
        // take tens of gigabytes before the first timed out. Each taken, checked, and counted
        // in turn, so a file past the triangles one may hold is refused once it's past them.
        let Some(_turn) = on.turns.take(solid.id, on.stopped) else {
            return Err(Problems::from(session_closed()));
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
        let pending = on.kernel.submit(on.kernel.generation(), request);
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

/// `solids` as one STEP file, written by the STEP worker (`on.step`; docs/ports/step-export.md): each a
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
    let (file, writer) = on.step.export_step(export, &|_| {}).map_err(|e| {
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
/// feature and the elements at fault by its element map, OCCT's words its cause; and other
/// faces, or face bounds (a face's outer edge and its holes), in the file than the parts
/// have that bound material (fewer: the writer drops what it can't map with only a warning).
/// One part's, each names its feature. None if it's whole.
fn unwritten(
    model: &Model,
    solids: &[Solid<'_>],
    file: &silico_protocol::StepFile,
) -> Option<Problems> {
    let off = |count: silico_protocol::StepCount| count.written != count.expected;
    if file.unwritten.is_empty() && !off(file.faces) && !off(file.bounds) {
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
        // OCCT's messages, each once, in the order first given (one for each element at
        // fault, often the same), and as many as the elements named, with how many more.
        // Seen in a set, so a reply of many is read once through (a log search each).
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut messages: Vec<&str> = Vec::new();
        for message in entries
            .iter()
            .flat_map(|e| e.messages.iter().map(String::as_str))
        {
            if seen.insert(message) && messages.len() < silico_protocol::FACES_NAMED {
                messages.push(message);
            }
        }
        let unsaid = seen.len() - messages.len();
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
        .with_cause(match (messages.is_empty(), unsaid) {
            (true, _) => "OCCT's check list names it, with no message".to_owned(),
            (false, 0) => messages.join("; "),
            (false, 1) => format!("{}; and 1 other message", messages.join("; ")),
            (false, n) => format!("{}; and {n} other messages", messages.join("; ")),
        })
        .with_suggestion(advice(solid));
        refused.push(named(error, solid));
    }
    for (count, what) in [(file.faces, "faces"), (file.bounds, "face bounds")] {
        if off(count) {
            let solid = of(None);
            let message = if count.written < count.expected {
                format!(
                    "the STEP file holds {} of the {} {what} the bodies have that bound \
                     material: OCCT's writer dropped {}",
                    count.written,
                    count.expected,
                    count.expected - count.written
                )
            } else {
                format!(
                    "the STEP file holds {} {what} where the bodies have {} that bound \
                     material: OCCT's writer didn't write them as they are",
                    count.written, count.expected
                )
            };
            let error = Error::new(ErrorCode::InvalidResult, message)
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::{DEFAULT_TOLERANCE, Line, MeshTurns, TOLERANCES};
    use crate::document::FeatureId;

    /// Long enough for any wait here, so one that stalls fails instead.
    const STALL: Duration = Duration::from_secs(60);

    fn feature() -> FeatureId {
        FeatureId::new("k7f2qx3m").unwrap()
    }

    /// Waits until `count` exports wait in `turns`' line.
    fn wait_in_line(turns: &MeshTurns, count: usize) {
        let deadline = Instant::now() + STALL;
        while crate::worker::lock(&turns.line).waiting.len() != count {
            assert!(Instant::now() < deadline, "{count} never waited");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A thread waiting for a turn in `turns` as `label`, stopped once `stop` is set: once it
    /// has the turn, `label` added to `order`, and the turn given back. Started once `ahead`
    /// wait, and returned once it waits too.
    fn waiter(
        turns: &Arc<MeshTurns>,
        label: &'static str,
        stop: &Arc<AtomicBool>,
        order: &Arc<Mutex<Vec<&'static str>>>,
        ahead: usize,
    ) -> std::thread::JoinHandle<bool> {
        wait_in_line(turns, ahead);
        let (turns_, stop, order) = (Arc::clone(turns), Arc::clone(stop), Arc::clone(order));
        let thread = std::thread::spawn(move || {
            let turn = turns_.take(&feature(), &|| stop.load(Ordering::SeqCst));
            let took = turn.is_some();
            if took {
                order.lock().unwrap().push(label);
            }
            took
        });
        wait_in_line(turns, ahead + 1);
        thread
    }

    #[test]
    fn turns_are_given_in_the_order_they_were_asked_for_the_holder_asking_again_behind() {
        // As the kernel pool serves its clients in turn: three exports wait, one after
        // another, while a turn is held; the holder gives it back and asks again at once,
        // ahead of every waiter's waking. Each waiter is served in the order it asked, and
        // the holder after them all.
        let turns = Arc::new(MeshTurns::new());
        let order = Arc::new(Mutex::new(Vec::new()));
        let never = Arc::new(AtomicBool::new(false));
        let held = turns.take(&feature(), &|| false).unwrap();
        let waiters: Vec<_> = ["first", "second", "third"]
            .into_iter()
            .enumerate()
            .map(|(ahead, label)| waiter(&turns, label, &never, &order, ahead))
            .collect();
        drop(held);
        let again = turns.take(&feature(), &|| false).unwrap();
        order.lock().unwrap().push("holder");
        drop(again);
        for waiter in waiters {
            assert!(waiter.join().unwrap());
        }
        assert_eq!(
            *order.lock().unwrap(),
            ["first", "second", "third", "holder"]
        );
        assert!(!crate::worker::lock(&turns.line).held);
    }

    #[test]
    fn a_waiter_stopped_mid_line_gives_up_its_place_and_the_one_behind_is_served() {
        // The middle of three waiters stopped, woken as a cancel wakes it: it leaves the line
        // with no turn, and the turn, given back, goes to the first and then the last, the
        // line never stalled on the place it left.
        let turns = Arc::new(MeshTurns::new());
        let order = Arc::new(Mutex::new(Vec::new()));
        let never = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let held = turns.take(&feature(), &|| false).unwrap();
        let first = waiter(&turns, "first", &never, &order, 0);
        let middle = waiter(&turns, "middle", &stop, &order, 1);
        let last = waiter(&turns, "last", &never, &order, 2);
        stop.store(true, Ordering::SeqCst);
        turns.wake();
        assert!(!middle.join().unwrap());
        assert_eq!(
            crate::worker::lock(&turns.line).waiting.len(),
            2,
            "the stopped waiter kept its place"
        );
        drop(held);
        let (done, joined) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send((first.join().unwrap(), last.join().unwrap()));
        });
        assert_eq!(joined.recv_timeout(STALL).unwrap(), (true, true));
        assert_eq!(*order.lock().unwrap(), ["first", "last"]);
    }

    #[test]
    fn a_first_waiter_stopped_as_the_turn_is_given_back_passes_it_to_the_one_behind() {
        // The first waiter stopped, unwoken, then the turn given back: whichever wakes first,
        // the stopped waiter leaves the line and the one behind it is served, never left
        // waiting for a turn nobody takes.
        let turns = Arc::new(MeshTurns::new());
        let order = Arc::new(Mutex::new(Vec::new()));
        let never = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let held = turns.take(&feature(), &|| false).unwrap();
        let first = waiter(&turns, "first", &stop, &order, 0);
        let behind = waiter(&turns, "behind", &never, &order, 1);
        stop.store(true, Ordering::SeqCst);
        drop(held);
        let (done, joined) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send((first.join().unwrap(), behind.join().unwrap()));
        });
        assert_eq!(joined.recv_timeout(STALL).unwrap(), (false, true));
        assert_eq!(*order.lock().unwrap(), ["behind"]);
        assert!(crate::worker::lock(&turns.line).waiting.is_empty());
    }

    /// A line whose turn is `held`, `waiting` its tickets in order, the next ticket past
    /// them all.
    fn a_line(held: bool, waiting: &[u64]) -> Line {
        Line {
            held,
            next: waiting.iter().max().map_or(0, |last| last + 1),
            waiting: waiting.iter().copied().collect(),
        }
    }

    /// `line`'s turn held, and its tickets waiting in order.
    fn state(line: &Line) -> (bool, Vec<u64>) {
        (line.held, line.waiting.iter().copied().collect())
    }

    #[test]
    fn a_newcomer_takes_a_free_turn_only_when_nobody_waits() {
        let mut free = a_line(false, &[]);
        assert!(free.take(None));
        assert_eq!(state(&free), (true, vec![]));
        // A turn held: no turn for it.
        let mut held = a_line(true, &[]);
        assert!(!held.take(None));
        assert_eq!(state(&held), (true, vec![]));
        // A turn given back but not yet taken by the line's first: a newcomer never goes
        // ahead of the exports waiting.
        let mut waited = a_line(false, &[4, 5]);
        assert!(!waited.take(None));
        assert_eq!(state(&waited), (false, vec![4, 5]));
    }

    #[test]
    fn only_the_first_in_line_takes_a_free_turn_and_gives_up_its_place() {
        let mut first = a_line(false, &[4, 5, 6]);
        assert!(first.take(Some(4)));
        assert_eq!(state(&first), (true, vec![5, 6]));
        // Behind the first, or at the back, whichever wakes first: no turn, the line kept.
        for behind in [5, 6] {
            let mut later = a_line(false, &[4, 5, 6]);
            assert!(!later.take(Some(behind)), "{behind} went ahead of 4");
            assert_eq!(state(&later), (false, vec![4, 5, 6]));
        }
        // First, but the turn held.
        let mut held = a_line(true, &[4, 5]);
        assert!(!held.take(Some(4)));
        assert_eq!(state(&held), (true, vec![4, 5]));
        // A ticket not in line at all (one that left).
        let mut gone = a_line(false, &[4]);
        assert!(!gone.take(Some(3)));
        assert_eq!(state(&gone), (false, vec![4]));
    }

    #[test]
    fn joining_gives_each_export_the_next_ticket_at_the_back() {
        let mut line = Line::default();
        assert_eq!((line.join(), line.join(), line.join()), (0, 1, 2));
        assert_eq!(state(&line), (false, vec![0, 1, 2]));
        // The ticket counter goes on past the tickets served: a ticket is never handed twice
        // to exports in line together.
        assert!(line.take(Some(0)));
        assert_eq!(line.join(), 3);
        assert_eq!(state(&line), (true, vec![1, 2, 3]));
    }

    #[test]
    fn leaving_first_with_the_turn_free_and_someone_behind_wakes_the_line() {
        // The turn given back, the first in line stopped before it woke to take it: the one
        // behind it may take it now, and is told.
        let mut front = a_line(false, &[4, 5, 6]);
        assert!(front.leave(4));
        assert_eq!(state(&front), (false, vec![5, 6]));
        let mut pair = a_line(false, &[4, 5]);
        assert!(pair.leave(4));
        assert_eq!(state(&pair), (false, vec![5]));
    }

    #[test]
    fn leaving_wakes_nobody_when_nobody_behind_could_take_a_turn() {
        // The first still first: from the middle or the back.
        for (leaving, kept) in [(5, vec![4, 6]), (6, vec![4, 5])] {
            let mut line = a_line(false, &[4, 5, 6]);
            assert!(!line.leave(leaving), "{leaving}");
            assert_eq!(state(&line), (false, kept));
        }
        // First, but the turn held: its giving back wakes the line.
        let mut held = a_line(true, &[4, 5]);
        assert!(!held.leave(4));
        assert_eq!(state(&held), (true, vec![5]));
        // First and alone: nobody to wake.
        let mut alone = a_line(false, &[4]);
        assert!(!alone.leave(4));
        assert_eq!(state(&alone), (false, vec![]));
    }

    #[test]
    fn giving_back_wakes_the_line_when_anyone_waits() {
        let mut waited = a_line(true, &[4, 5]);
        assert!(waited.give_back());
        assert_eq!(state(&waited), (false, vec![4, 5]));
        let mut nobody = a_line(true, &[]);
        assert!(!nobody.give_back());
        assert_eq!(state(&nobody), (false, vec![]));
    }

    #[test]
    fn a_first_waiter_leaving_a_free_turn_wakes_the_one_behind_to_take_it() {
        // The state a stopped first waiter leaves from, made by hand so no wake but its
        // leaving's can come: the turn free, a ticket first in line that no thread holds (the
        // stopped waiter's), and a thread waiting behind it. The waiter, woken only as the
        // ticket's place is given up as a stopped waiter's is, takes the turn; untold, it'd
        // wait for good.
        let turns = Arc::new(MeshTurns::new());
        let first = crate::worker::lock(&turns.line).join();
        let order = Arc::new(Mutex::new(Vec::new()));
        let never = Arc::new(AtomicBool::new(false));
        let behind = waiter(&turns, "behind", &never, &order, 1);
        turns.leave(&mut crate::worker::lock(&turns.line), first);
        let (done, joined) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(behind.join().unwrap());
        });
        assert!(
            joined.recv_timeout(STALL).unwrap(),
            "the waiter behind was never woken"
        );
        assert_eq!(*order.lock().unwrap(), ["behind"]);
        assert_eq!(state(&crate::worker::lock(&turns.line)), (false, vec![]));
    }

    #[test]
    fn an_export_stopped_before_it_waits_takes_no_turn_and_no_place() {
        let turns = MeshTurns::new();
        assert!(turns.take(&feature(), &|| true).is_none());
        let line = crate::worker::lock(&turns.line);
        assert!(!line.held && line.waiting.is_empty());
    }

    #[test]
    fn the_tolerances_version_gives_are_the_ones_an_export_takes() {
        assert_eq!(
            TOLERANCES.map(|t| t.mm()),
            [
                silico_protocol::MIN_EXPORT_TOLERANCE,
                silico_protocol::MAX_EXPORT_TOLERANCE
            ]
        );
    }

    #[test]
    fn the_default_tolerance_is_freecads() {
        assert_eq!(
            DEFAULT_TOLERANCE.mm(),
            silico_protocol::DEFAULT_EXPORT_TOLERANCE
        );
    }
}
