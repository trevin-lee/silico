//! Sketch-feature cases (M3.6, docs/design/m3.6-sketch-features.md, S9): pads, pockets,
//! revolves, and grooves, each a feature of one Silico document whose note ends with its
//! analytic expected values. FreeCAD 1.1.3 builds each case's features as its PartDesign
//! features (Pad, Pocket, Revolution, Groove, each with `Refine = False`, design S8) from
//! the same sketches, at the coordinates the document stores, and its measures are checked
//! against the expected values ([`against_expected`]): the cases are checked independently
//! of Silico. Silico rebuilds the document through the core's commands ([`silico`]), and its
//! measures are compared with FreeCAD's ([`compare`]) and with the expected values
//! ([`run`], which `silico-oracle run` reports).
//!
//! Pattern cases (M6.4, docs/design/m6.0-patterns.md, section 11) are a second document,
//! [`PATTERNS`], read the same way: each linear pattern, polar pattern, and mirror is a case
//! too, built by FreeCAD as PartDesign's `LinearPattern`, `PolarPattern`, and `Mirrored`
//! (`Refine = False`). A direction, axis, or plane that's an element's name is found in
//! FreeCAD's build by its geometry, as Silico's rebuild describes it ([`describe`]), as a
//! measure case's elements are.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use silico_core::Error;
use silico_core::command::rebuild;
use silico_core::document::{
    Axis, Checked, Copies, Feature, FeatureId, LinearDistribution, Operation, OriginAxis,
    OriginOrName, OriginPlane, PadExtent, Plane, PocketExtent, PolarDistribution, Position, Sketch,
    TurnExtent, read, skipped_in_range,
};
use silico_core::kernel::KernelWorker;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute};
use silico_protocol::arc_from_points;

use crate::compare::{Measured, Outcome, Tolerance, boxes_close, close, differences, size};
use crate::freecad;
use crate::measures::{self, Element};

/// The file of sketch-feature cases in an oracle directory: a Silico document.
pub const FILE: &str = "sketch-features.silico";

/// The file of pattern cases in an oracle directory (M6.4): a Silico document, whose linear
/// patterns, polar patterns, and mirrors are cases as its pads, pockets, revolves, and
/// grooves are.
pub const PATTERNS: &str = "patterns.silico";

/// What a case's note says its feature's solid measures, after `Expected: `: `volume V`,
/// and as the source gives them `faces F`, `edges E`, `vertices N`, `box X0 Y0 Z0 X1 Y1 Z1`
/// (the tight box's corners), and `center X Y Z` (of mass), separated by `; `, ending with a
/// full stop.
pub const EXPECTED: &str = "Expected: ";

/// What a case's solid should measure, from an independent source: a formula, or FreeCAD's
/// own test of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Expected {
    pub volume: f64,
    pub faces: Option<u32>,
    pub edges: Option<u32>,
    pub vertices: Option<u32>,
    pub bounds: Option<[[f64; 3]; 2]>,
    pub center: Option<[f64; 3]>,
}

impl Expected {
    /// The values `note` ends with, after [`EXPECTED`].
    pub fn parse(note: &str) -> Result<Expected, String> {
        let Some((_, values)) = note.split_once(EXPECTED) else {
            return Err(format!(
                "its note doesn't say what it's expected to measure (\"{EXPECTED}...\")"
            ));
        };
        let Some(values) = values.strip_suffix('.') else {
            return Err("its expected values don't end with a full stop".to_owned());
        };
        let mut found: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        for item in values.split("; ") {
            let mut words = item.split(' ');
            let key = words.next().unwrap_or_default();
            let numbers = words
                .map(|w| {
                    w.parse::<f64>()
                        .ok()
                        .filter(|x| x.is_finite())
                        .ok_or_else(|| format!("{key}'s value {w:?} isn't a finite number"))
                })
                .collect::<Result<Vec<f64>, String>>()?;
            let want = match key {
                "volume" | "faces" | "edges" | "vertices" => 1,
                "box" => 6,
                "center" => 3,
                _ => return Err(format!("{key:?} isn't something a case expects")),
            };
            if numbers.len() != want {
                return Err(format!("{key} takes {want} numbers, not {}", numbers.len()));
            }
            if found.insert(key, numbers).is_some() {
                return Err(format!("{key} is expected twice"));
            }
        }
        let count = |key: &str| -> Result<Option<u32>, String> {
            found
                .get(key)
                .map(|n| {
                    let n = n[0];
                    // A count: a whole number that fits.
                    if n.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&n) {
                        Ok(n as u32)
                    } else {
                        Err(format!("{key} {n} isn't a count"))
                    }
                })
                .transpose()
        };
        let volume = found
            .get("volume")
            .map(|v| v[0])
            .ok_or_else(|| "its expected values don't give its volume".to_owned())?;
        Ok(Expected {
            volume,
            faces: count("faces")?,
            edges: count("edges")?,
            vertices: count("vertices")?,
            bounds: found
                .get("box")
                .map(|b| [[b[0], b[1], b[2]], [b[3], b[4], b[5]]]),
            center: found.get("center").map(|c| [c[0], c[1], c[2]]),
        })
    }
}

/// One case: a pad, pocket, revolve, or groove of the document.
#[derive(Debug, Clone)]
pub struct FeatureCase {
    /// The feature's name.
    pub name: String,
    pub note: String,
    pub feature: FeatureId,
    pub expected: Expected,
    /// The features FreeCAD builds it from, in timeline order, in its terms.
    pub model: Vec<PartDesignFeature>,
    /// How far the two sides' measures may be apart, and FreeCAD's from the expected
    /// values: by what the case's solid is made of, as [`Tolerance::of`] says for a shape.
    pub tolerance: Tolerance,
}

/// The document of cases, through the gate, and its cases in timeline order.
#[derive(Debug, Clone)]
pub struct Cases {
    pub document: Checked,
    pub cases: Vec<FeatureCase>,
}

/// The cases in `dir`'s [`FILE`]; none if it has none. Every pad, pocket, revolve, and
/// groove is a case and says what it's expected to measure; every solid is one of them or
/// is used by a later feature, so no solid goes unchecked.
pub fn load(dir: &Path) -> Result<Option<Cases>, String> {
    load_file(dir, FILE)
}

/// The cases in `dir`'s [`PATTERNS`], as [`load`] reads [`FILE`]'s, each linear pattern,
/// polar pattern, and mirror a case too; none if it has none.
pub fn load_patterns(dir: &Path) -> Result<Option<Cases>, String> {
    load_file(dir, PATTERNS)
}

/// The cases in `dir`'s `file`; none if it has none.
fn load_file(dir: &Path, file: &str) -> Result<Option<Cases>, String> {
    let path = dir.join(file);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("can't read {}: {e}", path.display())),
    };
    let at = |e: String| format!("{}: {e}", path.display());
    let document = read(&bytes).map_err(|e| at(e.to_string()))?;
    let checked = Checked::new(document).map_err(|invalid| {
        let errors: Vec<String> = invalid.errors().iter().map(ToString::to_string).collect();
        at(errors.join("; "))
    })?;
    let cases = cases(checked.document().features.as_slice()).map_err(at)?;
    Ok(Some(Cases {
        document: checked,
        cases,
    }))
}

/// Each case of `features`, a gated document's.
fn cases(features: &[Feature]) -> Result<Vec<FeatureCase>, String> {
    let index: BTreeMap<&FeatureId, usize> = features
        .iter()
        .enumerate()
        .map(|(i, f)| (&f.id, i))
        .collect();
    let used: BTreeSet<&FeatureId> = features
        .iter()
        .flat_map(|f| f.operation.inputs().into_iter().map(|(_, id)| id))
        .collect();
    let mut names = BTreeSet::new();
    let mut found = Vec::new();
    for feature in features {
        let id = &feature.id;
        let note = feature.note.clone().unwrap_or_default();
        if !is_case(&feature.operation) {
            if note.contains(EXPECTED) {
                return Err(format!(
                    "{id} isn't a pad, pocket, revolve, or groove, nor a pattern or mirror, so it isn't a case, but its note says what it's expected to measure"
                ));
            }
            if feature.operation.makes_solid() && !used.contains(id) {
                return Err(format!(
                    "{id} makes a solid that isn't a case and that no case uses"
                ));
            }
            continue;
        }
        let name = feature.name.as_str().to_owned();
        if !names.insert(name.clone()) {
            return Err(format!("two cases are named {name}"));
        }
        let expected = Expected::parse(&note).map_err(|e| format!("{id} ({name}): {e}"))?;
        let chain = chain(features, &index, id).map_err(|e| format!("{id} ({name}): {e}"))?;
        let model = chain
            .iter()
            .map(|f| part_design(f, features, &index))
            .collect::<Result<Vec<_>, String>>()
            .map_err(|e| format!("{id} ({name}): {e}"))?;
        found.push(FeatureCase {
            name,
            note,
            feature: id.clone(),
            expected,
            tolerance: tolerance(&chain, features, &index),
            model,
        });
    }
    Ok(found)
}

/// A pad, pocket, revolve, or groove, or a linear pattern, polar pattern, or mirror: what
/// a case is.
fn is_case(operation: &Operation) -> bool {
    operation.sketch().is_some()
        || matches!(
            operation,
            Operation::LinearPattern { .. }
                | Operation::PolarPattern { .. }
                | Operation::Mirror { .. }
        )
}

/// The PartDesign body FreeCAD builds the pad, pocket, revolve, groove, or pattern `id` of
/// `features` (a gated document's) as, as a case's model is: its chain, in FreeCAD's terms,
/// its references by name not yet described ([`describe_model`]).
pub(crate) fn part_design_model(
    features: &[Feature],
    id: &FeatureId,
) -> Result<Vec<PartDesignFeature>, String> {
    let index: BTreeMap<&FeatureId, usize> = features
        .iter()
        .enumerate()
        .map(|(i, f)| (&f.id, i))
        .collect();
    chain(features, &index, id)?
        .iter()
        .map(|f| part_design(f, features, &index))
        .collect()
}

/// The features `id` is built from, and it, in timeline order: its bases, back to a
/// primitive or a sweep with none, and the sketches each sweep uses. A case's bases are one
/// line, as a PartDesign body's features are.
fn chain<'a>(
    features: &'a [Feature],
    index: &BTreeMap<&FeatureId, usize>,
    id: &FeatureId,
) -> Result<Vec<&'a Feature>, String> {
    let place = |id: &FeatureId| {
        index
            .get(id)
            .copied()
            .ok_or_else(|| format!("{id} isn't a feature"))
    };
    let mut wanted = BTreeSet::new();
    let mut at = Some(place(id)?);
    while let Some(i) = at {
        wanted.insert(i);
        let feature = &features[i];
        if let Some(sketch) = feature.operation.sketch() {
            wanted.insert(place(sketch)?);
        }
        at = match &feature.operation {
            Operation::Pad { base, .. } | Operation::Revolve { base, .. } => {
                base.as_ref().map(place).transpose()?
            }
            // A pattern's originals are in its base's chain (the gate's rule), so reached.
            Operation::Pocket { base, .. }
            | Operation::Groove { base, .. }
            | Operation::LinearPattern { base, .. }
            | Operation::PolarPattern { base, .. }
            | Operation::Mirror { base, .. } => Some(place(base)?),
            Operation::Box { .. }
            | Operation::Cylinder { .. }
            | Operation::Sphere { .. }
            | Operation::Cone { .. }
            | Operation::Torus { .. } => None,
            other => {
                return Err(format!(
                    "{} is a {}, which a feature case doesn't build",
                    feature.id,
                    other.type_name()
                ));
            }
        };
    }
    Ok(wanted.into_iter().map(|i| &features[i]).collect())
}

/// The case's tolerance: a sweep of a sketch with an arc or circle, a revolve, a groove, or
/// a primitive other than a box is curved, as [`Tolerance::of`] has a shape; a boolean
/// (a sweep onto a base) with a curved operand makes it a curved boolean.
fn tolerance(
    chain: &[&Feature],
    features: &[Feature],
    index: &BTreeMap<&FeatureId, usize>,
) -> Tolerance {
    let curved_sketch = |id: &FeatureId| {
        index.get(id).is_some_and(|&i| {
            matches!(&features[i].operation, Operation::Sketch(s) if !s.circles.is_empty() || !s.arcs.is_empty())
        })
    };
    let (mut curved, mut curved_boolean) = (false, false);
    for feature in chain {
        let own = match &feature.operation {
            Operation::Sketch(_) => continue,
            Operation::Box { .. } => false,
            Operation::Revolve { .. } | Operation::Groove { .. } => true,
            Operation::Pad { sketch, .. } | Operation::Pocket { sketch, .. } => {
                curved_sketch(sketch)
            }
            // A pattern's copies are of what's in its chain, already counted; the boolean
            // they're fused or cut in is, as any based feature's.
            Operation::LinearPattern { .. }
            | Operation::PolarPattern { .. }
            | Operation::Mirror { .. } => false,
            _ => true,
        };
        let based = !feature.operation.inputs().is_empty();
        if based && (curved || own) {
            curved_boolean = true;
        }
        curved |= own;
    }
    if curved_boolean {
        Tolerance::CURVED_BOOLEAN
    } else if curved {
        Tolerance::CURVED
    } else {
        Tolerance::PLANAR
    }
}

/// A feature of a case in FreeCAD's terms, for `oracle.py`'s `part_design`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartDesignFeature {
    /// A primitive, made by Part's `make<Kind>` at `position`, the body's base feature.
    Primitive {
        id: String,
        shape: &'static str,
        position: [f64; 3],
        values: Vec<f64>,
    },
    /// A sketch attached to the body's origin plane (`XY_Plane`, `XZ_Plane`, `YZ_Plane`),
    /// its geometry at the coordinates the document stores and no constraints.
    Sketch {
        id: String,
        plane: &'static str,
        geometry: Vec<SketchGeometry>,
    },
    /// A Pad or Pocket: `side_type` and `method` are FreeCAD's `SideType` and `Type`.
    Pad(Extrusion),
    Pocket(Extrusion),
    /// A Revolution or Groove of `angle` degrees about `axis`, as FreeCAD 1.1.3 has them
    /// (`Type` Angle, `Midplane`, `Reversed`).
    Revolution(Revolution),
    Groove(Revolution),
    /// A LinearPattern, PolarPattern, or Mirrored (PartDesign's `Transformed`), unrefined.
    LinearPattern(LinearPattern),
    PolarPattern(PolarPattern),
    Mirrored(Mirrored),
}

/// A LinearPattern: its originals, and `Direction`'s properties, then `Direction2`'s where
/// there's a second direction.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinearPattern {
    pub id: String,
    pub originals: Originals,
    pub directions: Vec<PatternDirection>,
}

/// What a pattern copies: its originals' tools, by ID in timeline order (the order FreeCAD
/// fuses and cuts them in, and Silico's steps run in), or `TransformMode` "Whole shape",
/// the base's whole solid.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Originals {
    Features(Vec<String>),
    WholeShape,
}

/// One direction of a LinearPattern, in FreeCAD's terms.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PatternDirection {
    pub reference: Reference,
    pub reversed: bool,
    /// `Mode`: `Extent` or `Spacing`.
    pub mode: &'static str,
    /// `Length` in Extent mode, from the first instance to the last, or `Offset` in Spacing
    /// mode, between neighbours, in mm.
    pub length: f64,
    pub occurrences: u32,
}

/// A PolarPattern, in FreeCAD's terms.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PolarPattern {
    pub id: String,
    pub originals: Originals,
    pub axis: Reference,
    pub reversed: bool,
    /// `Mode`: `Extent` or `Spacing`.
    pub mode: &'static str,
    /// `Angle` in Extent mode, or `Offset` in Spacing mode, in degrees.
    pub angle: f64,
    pub occurrences: u32,
}

/// A Mirrored, in FreeCAD's terms.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mirrored {
    pub id: String,
    pub originals: Originals,
    pub plane: Reference,
}

/// What a pattern's direction, axis, or plane refers to, in FreeCAD's terms.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reference {
    /// One of the body's origin features, by its role (`X_Axis`, `XY_Plane`).
    Origin { role: &'static str },
    /// An element of the solid of the feature `of` (the pattern's base), by its name in the
    /// document. FreeCAD finds it in its own build of `of` by its geometry, as Silico's
    /// rebuild describes it (`element`, which [`describe`] gives), never by its number,
    /// which the two builds needn't share.
    ///
    /// Where the two programs give it opposite senses, the harness turns FreeCAD's
    /// `Reversed` round (`corrected`), as a FreeCAD import corrects each once
    /// (docs/ports/patterns.md, decision 3): a face's direction is its outward normal in
    /// Silico and its surface's axis in FreeCAD; an edge's is `sense`, as Silico's
    /// `describe` gives it (a line from its start to its end, a circle's axis, which follow
    /// the way the profile walks its loop, M3.5), and its curve's own in FreeCAD (as the
    /// sketch drew it). Only a test that shows the difference turns `corrected` off.
    Element {
        of: String,
        name: String,
        element: Option<Element>,
        sense: Option<[f64; 3]>,
        corrected: bool,
    },
}

/// A sketch's curve in FreeCAD's terms, by its ID in the document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "curve", rename_all = "snake_case")]
pub enum SketchGeometry {
    Line {
        id: String,
        start: [f64; 2],
        end: [f64; 2],
        construction: bool,
    },
    Circle {
        id: String,
        center: [f64; 2],
        radius: f64,
        construction: bool,
    },
    /// Counter-clockwise from `from` to `to` (radians), as Silico's arc is.
    Arc {
        id: String,
        center: [f64; 2],
        radius: f64,
        from: f64,
        to: f64,
        construction: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Extrusion {
    pub id: String,
    pub sketch: String,
    pub side_type: &'static str,
    pub method: &'static str,
    pub length: f64,
    pub length2: f64,
    pub reversed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Revolution {
    pub id: String,
    pub sketch: String,
    pub axis: RevolutionAxis,
    pub angle: f64,
    pub midplane: bool,
    pub reversed: bool,
}

/// What a Revolution or Groove turns about: the sketch's own axis (`H_Axis`, `V_Axis`), or a
/// line of it, by its ID (a construction line is its `AxisN`, any other its edge).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevolutionAxis {
    Sketch(&'static str),
    Line(String),
}

/// `feature` in FreeCAD's terms.
fn part_design(
    feature: &Feature,
    features: &[Feature],
    index: &BTreeMap<&FeatureId, usize>,
) -> Result<PartDesignFeature, String> {
    let id = feature.id.as_str().to_owned();
    let place = |p: &Position| [p[0].mm(), p[1].mm(), p[2].mm()];
    let sketch_of = |s: &FeatureId| -> Result<&Sketch, String> {
        match index.get(s).map(|&i| &features[i].operation) {
            Some(Operation::Sketch(sketch)) => Ok(sketch),
            _ => Err(format!("{s} isn't a sketch")),
        }
    };
    let primitive = |shape: &'static str, position, values: Vec<f64>| {
        Ok(PartDesignFeature::Primitive {
            id: id.clone(),
            shape,
            position: place(position),
            values,
        })
    };
    match &feature.operation {
        Operation::Box {
            position,
            length,
            width,
            height,
        } => primitive("box", position, vec![length.mm(), width.mm(), height.mm()]),
        Operation::Cylinder {
            position,
            radius,
            height,
        } => primitive("cylinder", position, vec![radius.mm(), height.mm()]),
        Operation::Sphere { position, radius } => primitive("sphere", position, vec![radius.mm()]),
        Operation::Cone {
            position,
            radius1,
            radius2,
            height,
        } => primitive(
            "cone",
            position,
            vec![radius1.mm(), radius2.mm(), height.mm()],
        ),
        Operation::Torus {
            position,
            radius1,
            radius2,
        } => primitive("torus", position, vec![radius1.mm(), radius2.mm()]),
        Operation::Sketch(sketch) => Ok(PartDesignFeature::Sketch {
            id,
            plane: match &sketch.plane {
                Plane::Origin(plane) => origin_plane(*plane),
                // FreeCAD's FlatFace attachment, with the AttachmentOffset that turns its x onto
                // the token's (docs/ports/attacher.md, *Tests to port*), isn't in the harness yet.
                Plane::Face(_) => {
                    return Err(format!(
                        "{} is a sketch on a face, which the oracle harness doesn't build in FreeCAD yet",
                        feature.id
                    ));
                }
            },
            geometry: geometry(sketch)?,
        }),
        Operation::Pad { sketch, extent, .. } => {
            sketch_of(sketch)?;
            let (side_type, length, length2, reversed) = match *extent {
                PadExtent::OneSide { length, reversed } => ("One side", length, None, reversed),
                PadExtent::Symmetric { length } => ("Symmetric", length, None, false),
                PadExtent::TwoSides { length, length2 } => {
                    ("Two sides", length, Some(length2), false)
                }
            };
            Ok(PartDesignFeature::Pad(Extrusion {
                id,
                sketch: sketch.as_str().to_owned(),
                side_type,
                method: "Length",
                length: length.mm(),
                length2: length2.map_or(0.0, |l| l.mm()),
                reversed,
            }))
        }
        Operation::Pocket { sketch, extent, .. } => {
            sketch_of(sketch)?;
            let (side_type, method, length, length2, reversed) = match *extent {
                PocketExtent::OneSide { length, reversed } => {
                    ("One side", "Length", length.mm(), 0.0, reversed)
                }
                PocketExtent::Symmetric { length } => {
                    ("Symmetric", "Length", length.mm(), 0.0, false)
                }
                PocketExtent::TwoSides { length, length2 } => {
                    ("Two sides", "Length", length.mm(), length2.mm(), false)
                }
                PocketExtent::ThroughAll { reversed } => {
                    ("One side", "ThroughAll", 0.0, 0.0, reversed)
                }
            };
            Ok(PartDesignFeature::Pocket(Extrusion {
                id,
                sketch: sketch.as_str().to_owned(),
                side_type,
                method,
                length,
                length2,
                reversed,
            }))
        }
        Operation::Revolve {
            sketch,
            axis,
            extent,
            ..
        }
        | Operation::Groove {
            sketch,
            axis,
            extent,
            ..
        } => {
            let s = sketch_of(sketch)?;
            let axis = match axis {
                Axis::XAxis => RevolutionAxis::Sketch("H_Axis"),
                Axis::YAxis => RevolutionAxis::Sketch("V_Axis"),
                Axis::Line(line) => {
                    if !s.lines.iter().any(|l| &l.id == line) {
                        return Err(format!("{line} isn't a line of {sketch}"));
                    }
                    RevolutionAxis::Line(line.as_str().to_owned())
                }
            };
            let (angle, midplane, reversed) = match *extent {
                TurnExtent::OneSide { angle, reversed } => (angle, false, reversed),
                TurnExtent::Symmetric { angle } => (angle, true, false),
            };
            let revolution = Revolution {
                id,
                sketch: sketch.as_str().to_owned(),
                axis,
                angle: angle.degrees(),
                midplane,
                reversed,
            };
            Ok(match feature.operation {
                Operation::Revolve { .. } => PartDesignFeature::Revolution(revolution),
                _ => PartDesignFeature::Groove(revolution),
            })
        }
        Operation::LinearPattern {
            base,
            copies,
            directions,
            skip,
        } => {
            let counts: Vec<u32> = directions.iter().map(|d| d.count).collect();
            unskipped(feature, skip, &counts)?;
            let directions = directions
                .iter()
                .map(|d| {
                    let (mode, length) = match d.distribution {
                        LinearDistribution::Extent { length } => ("Extent", length),
                        LinearDistribution::Spacing { length } => ("Spacing", length),
                    };
                    PatternDirection {
                        reference: match &d.along {
                            OriginOrName::Origin(axis) => origin_axis(*axis),
                            OriginOrName::Name(name) => element(base, name.as_str()),
                        },
                        reversed: d.reversed,
                        mode,
                        length: length.mm(),
                        occurrences: d.count,
                    }
                })
                .collect();
            Ok(PartDesignFeature::LinearPattern(LinearPattern {
                id,
                originals: originals(copies, index)?,
                directions,
            }))
        }
        Operation::PolarPattern {
            base,
            copies,
            axis,
            reversed,
            distribution,
            count,
            skip,
        } => {
            unskipped(feature, skip, &[*count])?;
            let (mode, angle) = match *distribution {
                PolarDistribution::Extent { angle } => {
                    let degrees = angle.degrees();
                    // PolarPattern takes an extent within Precision::Confusion() of 360 as a
                    // whole turn (PP:270-280); Silico takes exactly 360 alone (decision 5).
                    if degrees != 360.0 && (degrees - 360.0).abs() < 1e-7 {
                        return Err(format!(
                            "{} turns {degrees} degrees, which FreeCAD 1.1.3 takes as a whole turn and Silico doesn't (docs/ports/patterns.md, decision 5)",
                            feature.id
                        ));
                    }
                    ("Extent", degrees)
                }
                PolarDistribution::Spacing { angle } => ("Spacing", angle.degrees()),
            };
            Ok(PartDesignFeature::PolarPattern(PolarPattern {
                id,
                originals: originals(copies, index)?,
                axis: match axis {
                    OriginOrName::Origin(axis) => origin_axis(*axis),
                    OriginOrName::Name(name) => element(base, name.as_str()),
                },
                reversed: *reversed,
                mode,
                angle,
                occurrences: *count,
            }))
        }
        Operation::Mirror {
            base,
            copies,
            plane,
        } => Ok(PartDesignFeature::Mirrored(Mirrored {
            id,
            originals: originals(copies, index)?,
            plane: match plane {
                OriginOrName::Origin(plane) => Reference::Origin {
                    role: origin_plane(*plane),
                },
                OriginOrName::Name(name) => element(base, name.as_str()),
            },
        })),
        other => Err(format!(
            "{} is a {}, which a feature case doesn't build",
            feature.id,
            other.type_name()
        )),
    }
}

/// The body's origin feature for `axis`.
fn origin_axis(axis: OriginAxis) -> Reference {
    Reference::Origin {
        role: match axis {
            OriginAxis::X => "X_Axis",
            OriginAxis::Y => "Y_Axis",
            OriginAxis::Z => "Z_Axis",
        },
    }
}

/// The body's origin feature for `plane`, as a sketch is attached to it.
fn origin_plane(plane: OriginPlane) -> &'static str {
    match plane {
        OriginPlane::Xy => "XY_Plane",
        OriginPlane::Xz => "XZ_Plane",
        OriginPlane::Yz => "YZ_Plane",
    }
}

/// The element `name` of `base`'s solid, not yet described.
fn element(base: &FeatureId, name: &str) -> Reference {
    Reference::Element {
        of: base.as_str().to_owned(),
        name: name.to_owned(),
        element: None,
        sense: None,
        corrected: true,
    }
}

/// What `copies` copies, in FreeCAD's terms: its features in timeline order.
fn originals(copies: &Copies, index: &BTreeMap<&FeatureId, usize>) -> Result<Originals, String> {
    match copies {
        Copies::Solid => Ok(Originals::WholeShape),
        Copies::Features { features } => {
            let mut placed = features
                .iter()
                .map(|id| {
                    index
                        .get(id)
                        .map(|&i| (i, id.as_str().to_owned()))
                        .ok_or_else(|| format!("{id} isn't a feature"))
                })
                .collect::<Result<Vec<_>, String>>()?;
            placed.sort();
            Ok(Originals::Features(
                placed.into_iter().map(|(_, id)| id).collect(),
            ))
        }
    }
}

/// Refuses a pattern that skips an instance it has: FreeCAD 1.1.3 has no instances to skip
/// (main's `SuppressedIndices` came after it). A skip past a count takes nothing out.
fn unskipped(
    feature: &Feature,
    skip: &[silico_core::document::Skipped],
    counts: &[u32],
) -> Result<(), String> {
    match skipped_in_range(skip, counts).first() {
        None => Ok(()),
        Some(instance) => Err(format!(
            "{} skips instance {instance}, which FreeCAD 1.1.3's patterns can't",
            feature.id
        )),
    }
}

/// Each of `cases` with its references by name described as Silico's rebuild `model` found
/// them, so FreeCAD can find each in its own build by its geometry ([`measures::resolve`],
/// as a measure case's elements are); or why a case's can't be, in its words.
pub fn describe(cases: &[FeatureCase], model: &Model) -> Vec<Result<FeatureCase, String>> {
    cases
        .iter()
        .map(|case| {
            let mut case = case.clone();
            describe_model(&mut case.model, model).map_err(|e| format!("{}: {e}", case.name))?;
            Ok(case)
        })
        .collect()
}

/// The references by name of a body's `features`, each described as [`describe`] does a
/// case's.
pub(crate) fn describe_model(
    features: &mut [PartDesignFeature],
    model: &Model,
) -> Result<(), String> {
    for feature in features {
        for reference in feature.references_mut() {
            if let Reference::Element {
                of,
                name,
                element,
                sense,
                ..
            } = reference
            {
                let of = FeatureId::new(of.as_str())
                    .ok_or_else(|| format!("{of} isn't a feature's ID"))?;
                *element = Some(measures::resolve(model, std::slice::from_ref(&of), name)?.element);
                *sense = measures::sense(model, &of, name)?;
            }
        }
    }
    Ok(())
}

impl PartDesignFeature {
    /// A pattern's direction, axis, or plane references; none for any other feature.
    pub fn references(&self) -> Vec<&Reference> {
        match self {
            PartDesignFeature::LinearPattern(p) => {
                p.directions.iter().map(|d| &d.reference).collect()
            }
            PartDesignFeature::PolarPattern(p) => vec![&p.axis],
            PartDesignFeature::Mirrored(m) => vec![&m.plane],
            _ => Vec::new(),
        }
    }

    fn references_mut(&mut self) -> Vec<&mut Reference> {
        match self {
            PartDesignFeature::LinearPattern(p) => {
                p.directions.iter_mut().map(|d| &mut d.reference).collect()
            }
            PartDesignFeature::PolarPattern(p) => vec![&mut p.axis],
            PartDesignFeature::Mirrored(m) => vec![&mut m.plane],
            _ => Vec::new(),
        }
    }
}

/// `cases` with their references by name described ([`describe`]) by Silico's rebuild of
/// their document, ready for [`freecad`]; the first that can't be, why.
pub fn described(cases: &Cases) -> Result<Vec<FeatureCase>, String> {
    let model = rebuilt(cases)?;
    describe(&cases.cases, &model).into_iter().collect()
}

/// `sketch`'s curves in FreeCAD's terms, at the coordinates it stores.
fn geometry(sketch: &Sketch) -> Result<Vec<SketchGeometry>, String> {
    let at: BTreeMap<_, [f64; 2]> = sketch
        .points
        .iter()
        .map(|p| (&p.id, [p.x.mm(), p.y.mm()]))
        .collect();
    let place = |id| {
        at.get(id)
            .copied()
            .ok_or_else(|| format!("a curve names point {id}, which the sketch doesn't have"))
    };
    let mut geometry = Vec::new();
    for l in &sketch.lines {
        geometry.push(SketchGeometry::Line {
            id: l.id.as_str().to_owned(),
            start: place(&l.from)?,
            end: place(&l.to)?,
            construction: l.construction,
        });
    }
    for c in &sketch.circles {
        geometry.push(SketchGeometry::Circle {
            id: c.id.as_str().to_owned(),
            center: place(&c.center)?,
            radius: c.radius.mm(),
            construction: c.construction,
        });
    }
    for a in &sketch.arcs {
        let [radius, from, to] =
            arc_from_points(place(&a.center)?, place(&a.start)?, place(&a.end)?)
                .ok_or_else(|| format!("arc {} sweeps nothing", a.id))?;
        geometry.push(SketchGeometry::Arc {
            id: a.id.as_str().to_owned(),
            center: place(&a.center)?,
            radius,
            from,
            to,
            construction: a.construction,
        });
    }
    Ok(geometry)
}

/// A case as `oracle.py` takes it.
#[derive(Serialize)]
struct FreecadCase<'a> {
    name: &'a str,
    shape: FreecadShape<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum FreecadShape<'a> {
    PartDesign { features: &'a [PartDesignFeature] },
}

/// What FreeCAD builds and measures for each of `cases`, in order.
pub fn freecad(cases: &[FeatureCase]) -> Result<Vec<Outcome>, String> {
    let _span = tracing::info_span!("freecad_feature_cases", cases = cases.len()).entered();
    for case in cases {
        let undescribed = case.model.iter().find_map(|f| {
            f.references().into_iter().find_map(|r| match r {
                Reference::Element {
                    name,
                    element: None,
                    ..
                } => Some(name.clone()),
                _ => None,
            })
        });
        if let Some(name) = undescribed {
            return Err(format!(
                "{}'s reference {name} isn't described: FreeCAD finds it by its geometry as Silico's rebuild describes it (`describe`)",
                case.name
            ));
        }
    }
    let models: Vec<FreecadCase> = cases
        .iter()
        .map(|c| FreecadCase {
            name: &c.name,
            shape: FreecadShape::PartDesign { features: &c.model },
        })
        .collect();
    freecad::run(&models)
}

/// What became of a case in Silico's rebuild.
#[derive(Debug, Clone)]
pub enum Ours {
    /// Built, and measured as FreeCAD measures.
    Built(Measured),
    /// Refused: why.
    Failed(Error),
    /// Not built, since it uses this feature, which failed.
    Blocked(FeatureId),
}

impl Ours {
    /// As the oracle compares outcomes: a refusal or a block that's the kernel's own
    /// trouble is [`Outcome::Lost`].
    pub fn outcome(&self) -> Outcome {
        match self {
            Ours::Built(measured) => Outcome::Built(measured.clone()),
            Ours::Failed(e) if e.code().is_kernel_fault() => Outcome::Lost(e.to_string()),
            Ours::Failed(e) => Outcome::Failed(e.to_string()),
            Ours::Blocked(on) => Outcome::Failed(format!("blocked on {on}, which failed")),
        }
    }
}

/// The cases' document rebuilt through the core's commands, on a pool of kernel workers,
/// its sketches on the sketch worker.
pub fn rebuilt(cases: &Cases) -> Result<std::sync::Arc<Model>, String> {
    let engine = Recompute::new(KernelPool::start());
    rebuild(&engine, cases.document.clone())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the rebuild was superseded, though nothing else ran".to_owned())
}

/// Silico's outcome for each case: its solid in `model`, the cases' document rebuilt
/// ([`rebuilt`]), measured on a kernel worker as FreeCAD measures.
pub fn silico(cases: &Cases, model: &Model) -> Result<Vec<Ours>, String> {
    let _span = tracing::info_span!("silico_feature_cases", cases = cases.cases.len()).entered();
    let mut kernel = KernelWorker::start().map_err(|e| e.to_string())?;
    let mut found = Vec::with_capacity(cases.cases.len());
    for case in &cases.cases {
        let ours = match model.outcome(&case.feature) {
            Some(FeatureOutcome::Built { built, .. }) => {
                let measured = kernel.summarize(&built.shape).and_then(|summary| {
                    Ok(Measured::new(
                        &summary,
                        kernel.measure_as_freecad(&built.shape)?,
                    ))
                });
                match measured {
                    Ok(measured) => Ours::Built(measured),
                    Err(e) => {
                        if e.code().is_kernel_fault() {
                            kernel = KernelWorker::start().map_err(|e| e.to_string())?;
                        }
                        Ours::Failed(e)
                    }
                }
            }
            Some(FeatureOutcome::Failed(e)) => Ours::Failed(e.clone()),
            Some(FeatureOutcome::Broken { errors, .. }) => match errors.first() {
                Some(e) => Ours::Failed(e.clone()),
                None => return Err(format!("{} is broken, naming nothing", case.feature)),
            },
            Some(FeatureOutcome::Blocked { on }) => Ours::Blocked(on.clone()),
            Some(FeatureOutcome::Sketch(_) | FeatureOutcome::Suppressed) | None => {
                return Err(format!("the rebuild has no solid for {}", case.feature));
            }
        };
        found.push(ours);
    }
    Ok(found)
}

/// Every way FreeCAD's (or Silico's) `outcome` of `case` differs from its expected values,
/// to its tolerance; none if it agrees. Volume is relative; the box's corners and the
/// center are relative to the larger box's largest side (at least 1 mm), as the oracle
/// compares two sides; counts exactly; and the shape must be valid.
pub fn against_expected(case: &FeatureCase, outcome: &Outcome) -> Vec<String> {
    let measured = match outcome {
        Outcome::Built(measured) => measured,
        Outcome::Failed(why) => return vec![format!("failed: {why}")],
        Outcome::Lost(why) => return vec![format!("lost: {why}")],
    };
    let (want, tolerance) = (&case.expected, case.tolerance);
    let mut found = Vec::new();
    if !close(measured.volume, want.volume, tolerance.relative) {
        found.push(format!(
            "volume {} against the expected {}",
            measured.volume, want.volume
        ));
    }
    for (what, got, expected) in [
        ("faces", measured.faces, want.faces),
        ("edges", measured.edges, want.edges),
        ("vertices", measured.vertices, want.vertices),
    ] {
        if let Some(expected) = expected
            && got != expected
        {
            found.push(format!("{what} {got} against the expected {expected}"));
        }
    }
    if !measured.valid {
        found.push("not valid".to_owned());
    }
    if let Some(expected) = want.bounds {
        match measured.bounds {
            Some(got) if boxes_close(got, expected, tolerance.bounds) => {}
            got => found.push(format!("box {got:?} against the expected {expected:?}")),
        }
    }
    if let Some(expected) = want.center {
        let scale = measured
            .bounds
            .map_or(1.0, size)
            .max(want.bounds.map_or(1.0, size));
        match measured.center {
            Some(got)
                if (0..3).all(|k| (got[k] - expected[k]).abs() <= tolerance.center * scale) => {}
            got => found.push(format!("center {got:?} against the expected {expected:?}")),
        }
    }
    found
}

/// How Silico's outcome of `case` differs from FreeCAD's, to the case's tolerance.
pub fn compare(case: &FeatureCase, ours: &Outcome, theirs: &Outcome) -> Vec<String> {
    differences(ours, theirs, case.tolerance)
}

/// Every case of `cases` that differs: Silico's rebuild of the document (M3.6, step iv;
/// M6.4) compared with FreeCAD's build of each case, and with the case's expected values
/// (each such difference said as Silico's), for `silico-oracle run` and the test alike.
pub fn run(cases: &Cases) -> Result<Vec<crate::Difference>, String> {
    let model = rebuilt(cases)?;
    let ours = silico(cases, &model)?;
    // A case whose reference by name Silico's rebuild can't give (its base refused, or the
    // name finding nothing) isn't asked of FreeCAD: it's lost, a difference, and the rest
    // are still compared.
    let described = describe(&cases.cases, &model);
    let asked: Vec<FeatureCase> = described.iter().filter_map(|d| d.clone().ok()).collect();
    let answered = freecad(&asked)?;
    if ours.len() != cases.cases.len() || answered.len() != asked.len() {
        return Err(format!(
            "{} feature cases gave {} outcomes from Silico, and FreeCAD {} for the {} it was asked",
            cases.cases.len(),
            ours.len(),
            answered.len(),
            asked.len()
        ));
    }
    let mut answered = answered.into_iter();
    let theirs = described.into_iter().map(|d| match d {
        Ok(_) => answered.next().unwrap_or_else(|| Outcome::Lost("unanswered".to_owned())),
        Err(why) => Outcome::Lost(format!(
            "not built: its reference isn't in Silico's rebuild, so FreeCAD can't be shown it: {why}"
        )),
    });
    let mut found = Vec::new();
    for ((case, ours), theirs) in cases.cases.iter().zip(&ours).zip(theirs) {
        let ours = ours.outcome();
        let mut how = compare(case, &ours, &theirs);
        how.extend(
            against_expected(case, &ours)
                .into_iter()
                .map(|h| format!("Silico's {h}")),
        );
        if !how.is_empty() {
            found.push(crate::Difference {
                case: case.name.clone(),
                ours,
                theirs,
                how,
            });
        }
    }
    Ok(found)
}
