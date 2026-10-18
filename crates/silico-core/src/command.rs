//! The core's commands, which every client (the CLI, and MCP and the app as they arrive)
//! runs, so each does exactly what the others do: a document's bytes through the gate
//! ([`gate`]), rebuilt ([`rebuild`]), checked ([`check`]), written in canonical form with
//! its fingerprints ([`format`]), and its outline ([`outline`]). [`check`] and [`format`]
//! take a model, so a client with a live model (the app's, of the document it saves) uses
//! that one, and one with none rebuilds it. Each gives structured errors; reading and
//! writing files is the client's.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::document::{self, Checked, Document, Feature, Fingerprint, Operation};
use crate::kernel::Kernel;
use crate::recompute::{Model, Recompute, RunOutcome};
use crate::{Error, ErrorCode};

/// What was wrong with a document: every problem found, in the order found (reading, then
/// the gate, then each failed feature and each fingerprint that doesn't hold, in timeline
/// order), and how many there are, more than [`Problems::errors`] lists only when the gate
/// found too many to list.
#[derive(Debug, Clone, PartialEq)]
pub struct Problems {
    errors: Vec<Error>,
    total: u64,
}

impl Problems {
    fn of(errors: Vec<Error>) -> Self {
        let total = errors.len() as u64;
        Problems { errors, total }
    }

    /// The problems listed: every one, or the first [`document::PROBLEMS_LISTED`] the gate
    /// found.
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// How many there are, listed or not.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// `bytes` read and through the gate, or what's wrong with them: the first part of a check,
/// which needs no kernel, so a client can say what's wrong with a document it can't
/// rebuild.
pub fn gate(bytes: &[u8]) -> Result<Checked, Problems> {
    let document = document::read(bytes).map_err(|error| Problems::of(vec![error]))?;
    Checked::new(document).map_err(Problems::from)
}

impl From<document::Invalid> for Problems {
    fn from(invalid: document::Invalid) -> Self {
        Problems {
            errors: invalid.errors().to_vec(),
            total: invalid.total(),
        }
    }
}

/// A gated document rebuilt on `engine`, waited for: its model; or none, if a later
/// recompute on the engine superseded it (a client that shares its engine, as the app
/// will, has the newer one's model). An error is a recompute that stopped (a bug).
pub fn rebuild<K: Kernel + 'static>(
    engine: &Recompute<K>,
    checked: Checked,
) -> Result<Option<Arc<Model>>, Error> {
    let _span = tracing::info_span!("rebuild").entered();
    Ok(match engine.start(checked).wait()? {
        RunOutcome::Published(model) => Some(model),
        RunOutcome::Superseded => None,
    })
}

/// A model's problems: each failed feature, and each fingerprint the rebuild doesn't
/// match (`Model::drift`), if any. A feature blocked by another's failure isn't a problem
/// of its own (the failure at its root is). An error is a check that couldn't be done:
/// the kernel failed in a way that says nothing about the document (a worker that crashed
/// or hung, a shortage, a reply that couldn't be read), which a client reports as its own
/// failure, not the document's.
pub fn check(model: &Model) -> Result<Problems, Error> {
    let _span = tracing::info_span!("check").entered();
    let mut problems = failures(model)?;
    problems.extend(own(model.drift())?);
    Ok(Problems::of(problems))
}

/// The model's failed features, as problems of the document; or, if one failed for a
/// reason that isn't the document's, that.
fn failures(model: &Model) -> Result<Vec<Error>, Error> {
    own(model.errors().cloned().collect())
}

/// `errors`, the document's own problems; or the first that's the kernel's (a solid whose
/// measure timed out, say), which isn't.
fn own(errors: Vec<Error>) -> Result<Vec<Error>, Error> {
    match errors.iter().find(|e| e.code().is_kernel_fault()) {
        Some(fault) => Err(fault.clone()),
        None => Ok(errors),
    }
}

/// A document written in canonical form, its fingerprints as `engine` rebuilt it.
#[derive(Debug, Clone, PartialEq)]
pub struct Formatted {
    /// The canonical text.
    pub text: String,
    /// The features whose fingerprints the rebuild changed (written new, or rewritten),
    /// in timeline order, each with what was stored before, if anything.
    pub refingerprinted: Vec<(document::FeatureId, Option<Fingerprint>)>,
    /// The features that didn't build, or whose solids couldn't be measured: each keeps
    /// the fingerprint stored with it, and this says why. Drift isn't among them: it's
    /// what formatting fixes.
    pub problems: Problems,
}

/// The model's document in canonical form with each feature's fingerprint as the model
/// rebuilt it (`Model::fingerprinted`): what `silico fmt` writes, and what a save writes;
/// or, if its canonical text would be too long, that problem. An error is a format that
/// couldn't be done, as for [`check`]: nothing should be written then.
pub fn format(model: &Model) -> Result<Result<Formatted, Problems>, Error> {
    let _span = tracing::info_span!("format").entered();
    let mut problems = failures(model)?;
    let written = model.fingerprinted();
    let refingerprinted = model
        .document()
        .features
        .iter()
        .zip(&written.features)
        .filter(|(before, after)| before.fingerprint != after.fingerprint)
        .map(|(before, _)| (before.id.clone(), before.fingerprint))
        .collect();
    let text = match document::write(&written) {
        Ok(text) => text,
        Err(error) => return Ok(Err(Problems::of(vec![error]))),
    };
    // Problems the save can't fix: drift is what it fixes, so only the failures and the
    // solids that couldn't be measured.
    problems.extend(
        own(model.drift())?
            .into_iter()
            .filter(|error| error.code() != ErrorCode::Drift),
    );
    Ok(Ok(Formatted {
        text,
        refingerprinted,
        problems: Problems::of(problems),
    }))
}

/// The document's outline, for reading and for `git diff` (`silico show`): one line per
/// feature, in timeline order, saying what it is, its values as the file writes them,
/// and its fingerprint, if stored; its note, if any, on the line after; and the bodies
/// the timeline ends with. Nothing is aligned in columns, so an edit to one feature
/// changes its lines alone.
pub fn outline(document: &Document) -> String {
    // Every field taken apart, so one added to a document or a feature is a compile error
    // here until the outline says it or says why not: what it leaves out, `git diff`
    // through it never shows. The format's version, the units, and a feature's version
    // each have one value so far.
    let Document {
        silico: _,
        units: _,
        features,
    } = document;
    let mut out = String::new();
    for feature in features {
        let Feature {
            id,
            operation,
            version: _,
            name,
            note,
            fingerprint,
        } = feature;
        let _ = write!(
            out,
            "{} {} {}",
            id,
            quoted(name.as_str()),
            operation.type_name()
        );
        let values = values(operation);
        if !values.is_empty() {
            let _ = write!(out, " {values}");
        }
        if let Some(fingerprint) = fingerprint {
            let _ = write!(out, " | {}", measures(fingerprint));
        }
        out.push('\n');
        if let Some(note) = note {
            let _ = writeln!(out, "  note {}", quoted(note));
        }
        // A sketch's points, curves, and constraints, a line each, as the file writes them,
        // so an edit to one changes its line alone.
        if let Operation::Sketch(sketch) = operation {
            for (kind, text) in document::sketch_elements(sketch) {
                let _ = writeln!(out, "  {kind} {text}");
            }
        }
    }
    let bodies = bodies(document);
    let _ = writeln!(
        out,
        "bodies {}",
        if bodies.is_empty() {
            "none".to_owned()
        } else {
            bodies.join(", ")
        }
    );
    out
}

/// A feature's values, as the file names and writes them.
fn values(operation: &Operation) -> String {
    let at = |p: &document::Position| format!("at ({}, {}, {})", p[0], p[1], p[2]);
    match operation {
        Operation::Box {
            position,
            length,
            width,
            height,
        } => format!(
            "length {length}, width {width}, height {height}, {}",
            at(position)
        ),
        Operation::Cylinder {
            position,
            radius,
            height,
        } => format!("radius {radius}, height {height}, {}", at(position)),
        Operation::Sphere { position, radius } => format!("radius {radius}, {}", at(position)),
        Operation::Cone {
            position,
            radius1,
            radius2,
            height,
        } => format!(
            "radius1 {radius1}, radius2 {radius2}, height {height}, {}",
            at(position)
        ),
        Operation::Torus {
            position,
            radius1,
            radius2,
        } => format!("radius1 {radius1}, radius2 {radius2}, {}", at(position)),
        Operation::Fuse { base, tool }
        | Operation::Cut { base, tool }
        | Operation::Common { base, tool } => format!("base {base}, tool {tool}"),
        // Its elements follow, a line each.
        Operation::Sketch(sketch) => format!("plane {}", sketch.plane.as_str()),
    }
}

/// A fingerprint's measures, as the file writes them.
fn measures(f: &Fingerprint) -> String {
    let bbox = match &f.bbox {
        Some(c) => format!(
            "box ({}, {}, {}) to ({}, {}, {})",
            c[0], c[1], c[2], c[3], c[4], c[5]
        ),
        None => "no box".to_owned(),
    };
    format!(
        "volume {}, area {}, {bbox}, {} faces, {} edges, {} vertices",
        f.volume, f.area, f.faces, f.edges, f.vertices
    )
}

/// The features that make a solid and that no later feature consumes, in timeline order:
/// the solids the timeline ends with, if every feature builds.
fn bodies(document: &Document) -> Vec<String> {
    let consumed: std::collections::BTreeSet<&document::FeatureId> = document
        .features
        .iter()
        .filter_map(|f| f.operation.inputs())
        .flat_map(|inputs| inputs.into_iter().map(|(_, id)| id))
        .collect();
    document
        .features
        .iter()
        // A sketch makes no solid, so it's no body.
        .filter(|f| f.operation.makes_solid() && !consumed.contains(&f.id))
        .map(|f| f.id.to_string())
        .collect()
}

/// `text` in double quotes, its quotes, backslashes, and control and invisible characters
/// escaped, so a name or note can't break the outline's lines or hide in them.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            // Controls, the line and paragraph separators, every character Unicode draws
            // as nothing, and every space but the plain one, which would look like it.
            c if c.is_control()
                || matches!(c, '\u{2028}' | '\u{2029}')
                || (c.is_whitespace() && c != ' ')
                || document::json::invisible(c) =>
            {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
