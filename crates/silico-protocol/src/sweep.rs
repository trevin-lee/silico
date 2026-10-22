//! Sweeps of a profile's faces into solids, for pad, pocket, revolve, and groove
//! (docs/design/m3.6-sketch-features.md, S3): a prism along a vector, from a start offset, or
//! through all of a base; and a revolution about an axis by an angle, from a start angle. The
//! core works out the exact geometry from the document's integers; the worker moves (or
//! turns) the faces to the start and sweeps them, and decides nothing. The rules for the
//! values, for the through-all length, and for the history a sweep reports live here, so the
//! core refuses a bad request before sending it, the worker refuses it again, and the core
//! refuses a reply that breaks them.

use std::collections::BTreeSet;
use std::f64::consts::TAU;

use crate::history::{ElementKind, History, InputHistory};
use crate::primitive::{InvalidParameter, LENGTH_TOLERANCE, MAX_LENGTH, format_number};

/// OCCT's angular tolerance (`Precision::Angular()`), in radians: a revolution of no more
/// than this is no revolution, and one within it of a whole turn is a whole turn
/// (`BRepSweep_Revol`).
pub const ANGULAR_TOLERANCE: f64 = 1e-12;

/// How much longer than the diagonal of the box around the base and the profile a
/// through-all prism is: FreeCAD's factor (`getThroughAllLength`, doubled from 1.01 by
/// `43de589732` so a symmetric through all goes all the way).
pub const THROUGH_ALL_FACTOR: f64 = 2.02;

/// The relative precision the precise measures (a summary's volume and area,
/// `BRepGProp::VolumeProperties` and `SurfaceProperties`) are integrated to. The kernel's
/// measure takes it from here, and a sweep feature that changes its base's volume by no more
/// than this much of it changes nothing (design S7), so neither can change alone.
pub const MEASURE_PRECISION: f64 = 1e-9;

fn invalid(parameter: &'static str, message: String) -> InvalidParameter {
    InvalidParameter { parameter, message }
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// That `point` is a place: each coordinate finite and at most [`MAX_LENGTH`] from 0.
fn check_point(
    parameter: &'static str,
    what: &str,
    point: [f64; 3],
) -> Result<(), InvalidParameter> {
    if point.iter().all(|c| c.is_finite() && c.abs() <= MAX_LENGTH) {
        Ok(())
    } else {
        Err(invalid(
            parameter,
            format!(
                "the {what} must have finite coordinates within 1e6 mm (a kilometre) of the origin, got {point:?}"
            ),
        ))
    }
}

/// That `direction` is a unit vector, to rounding: the core sends the sketch plane's axes
/// and lines' directions normalized, and the worker sweeps along exactly what it's sent.
fn check_unit(
    parameter: &'static str,
    what: &str,
    direction: [f64; 3],
) -> Result<(), InvalidParameter> {
    let size = length(direction);
    if direction.iter().all(|c| c.is_finite()) && (size - 1.0).abs() <= UNIT_SLACK {
        Ok(())
    } else {
        Err(invalid(
            parameter,
            format!("the {what} must be a unit vector, got {direction:?}"),
        ))
    }
}

/// How far a unit direction's length may be from 1: rounding, as the core normalizes one.
const UNIT_SLACK: f64 = 4.0 * f64::EPSILON;

/// The longest a sweep's vector may measure: a kilometre, and the rounding a unit direction
/// (to [`UNIT_SLACK`]) times a length of at most a kilometre, measured, can add to it. So a
/// through-all prism of a length [`through_all_length`] admits, along a direction
/// [`check_through_all`] admits, is a prism [`check_prism`] admits (break-it found the
/// kernel refusing one a few ulps past the kilometre, naming a `vector` the request didn't
/// have).
const MAX_SWEEP: f64 = MAX_LENGTH * (1.0 + 4.0 * UNIT_SLACK);

/// Whether a prism of a profile's faces moved by `start`, then swept along `vector`, can be
/// built: `start` a place ([`check_point`]'s rule), and `vector` finite, longer than OCCT's
/// tolerance, and at most a kilometre (to the rounding of a unit direction, [`MAX_SWEEP`]).
pub fn check_prism(start: [f64; 3], vector: [f64; 3]) -> Result<(), InvalidParameter> {
    check_point("start", "prism's start", start)?;
    let size = length(vector);
    if vector.iter().all(|c| c.is_finite()) && size > LENGTH_TOLERANCE && size <= MAX_SWEEP {
        Ok(())
    } else {
        Err(invalid(
            "vector",
            format!(
                "the prism's vector must be finite, longer than 1e-7 mm, and at most 1e6 mm (a kilometre), got {vector:?}"
            ),
        ))
    }
}

/// Whether a prism along `vector` leaves the plane of faces whose unit normal is `normal`:
/// the vector's unit direction makes at least FreeCAD's 1e-7 with the plane
/// (|direction · normal| ≥ 1e-7, M1's `extrusion_vector` rule), so it sweeps a solid with a
/// volume, not a flat one. The kernel reads the faces' normal; `parameter` names the field
/// the vector came from (`vector`, or a through-all prism's `direction`). The core's
/// vectors lie along the normal, so this is the kernel's own guard.
pub fn check_leaves_plane(
    parameter: &'static str,
    normal: [f64; 3],
    vector: [f64; 3],
) -> Result<(), InvalidParameter> {
    let size = length(vector);
    let along = (vector[0] * normal[0] + vector[1] * normal[1] + vector[2] * normal[2]).abs();
    if size > 0.0 && along >= crate::profile::MIN_DIRECTION_FACTOR * size {
        Ok(())
    } else {
        Err(invalid(
            parameter,
            format!(
                "the sweep's {parameter} {vector:?} lies in the faces' plane (normal {normal:?}): it must point out of it"
            ),
        ))
    }
}

/// Whether a revolution's axis, along the unit `direction`, lies in the plane of faces whose
/// unit normal is `normal`, to FreeCAD's 1e-7 (|direction · normal| ≤ 1e-7): an axis out of
/// the plane turns the faces through themselves, and one along the normal spins them in
/// their plane into a flat "solid". The core's axes are lines of the sketch, in its plane,
/// so this is the kernel's own guard.
pub fn check_axis_in_plane(normal: [f64; 3], direction: [f64; 3]) -> Result<(), InvalidParameter> {
    let lean =
        (direction[0] * normal[0] + direction[1] * normal[1] + direction[2] * normal[2]).abs();
    if lean <= crate::profile::MIN_DIRECTION_FACTOR {
        Ok(())
    } else {
        Err(invalid(
            "axis",
            format!(
                "the revolution's axis {direction:?} leans out of the faces' plane (normal {normal:?}): it must lie in it"
            ),
        ))
    }
}

/// Whether a through-all prism along `direction` can be asked for: a unit vector.
pub fn check_through_all(direction: [f64; 3]) -> Result<(), InvalidParameter> {
    check_unit("direction", "through-all prism's direction", direction)
}

/// How long a through-all prism is, from the tight box around its base and its profile
/// (`low` to `high`): [`THROUGH_ALL_FACTOR`] times the box's diagonal, FreeCAD's rule
/// (design S3). Refused past a kilometre (a base about 500 m across), as any sweep is, and
/// for a box with no extent, which no base and profile have.
pub fn through_all_length([low, high]: [[f64; 3]; 2]) -> Result<f64, InvalidParameter> {
    let diagonal = length([high[0] - low[0], high[1] - low[1], high[2] - low[2]]);
    let sweep = THROUGH_ALL_FACTOR * diagonal;
    if sweep > LENGTH_TOLERANCE && sweep <= MAX_LENGTH {
        Ok(sweep)
    } else {
        Err(invalid(
            "base",
            format!(
                "through all of the base and the profile, {} mm across, is {} mm: a sweep must be longer than 1e-7 mm and at most 1e6 mm (a kilometre)",
                format_number(diagonal),
                format_number(sweep)
            ),
        ))
    }
}

/// Whether a revolution of a profile's faces turned `start` radians about the axis through
/// `origin` along `direction`, then swept `angle` radians, can be built: the axis a place
/// and a unit vector, the start within a turn either way, and the angle above OCCT's
/// angular tolerance and at most a whole turn (360° by `radians`, FreeCAD's conversion).
pub fn check_revolution(
    origin: [f64; 3],
    direction: [f64; 3],
    start: f64,
    angle: f64,
) -> Result<(), InvalidParameter> {
    check_point("axis", "axis's point", origin)?;
    check_unit("axis", "axis's direction", direction)?;
    if !(start.is_finite() && start.abs() <= TAU) {
        return Err(invalid(
            "start",
            format!(
                "the revolution's start must be finite and within a turn (2π radians) of 0, got {}",
                format_number(start)
            ),
        ));
    }
    if !(angle > ANGULAR_TOLERANCE && angle <= crate::radians(360.0)) {
        return Err(invalid(
            "angle",
            format!(
                "the revolution's angle must be above 1e-12 radians and at most a turn, got {}",
                format_number(angle)
            ),
        ));
    }
    Ok(())
}

/// Whether a revolution of `angle` radians is a whole turn to OCCT: within its angular
/// tolerance of 2π (`BRepSweep_Revol`). A whole turn has no start and no end.
pub fn is_whole_turn(angle: f64) -> bool {
    (angle - TAU).abs() <= ANGULAR_TOLERANCE
}

/// How a turn's end lies within OCCT's length tolerance of its start ([`end_at_start`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndAtStart {
    /// The turn moves the profile's farthest point from the axis by no more than the
    /// tolerance: it makes no solid.
    Unmoved,
    /// The turn stops short of a whole one by no more than the tolerance at the profile's
    /// farthest point, and isn't a whole turn to OCCT ([`is_whole_turn`]): its ends are one
    /// place, yet OCCT builds them as two, an open ring a later cut across finds cutting
    /// itself.
    ShortOfWhole,
}

/// Whether a turn of `angle` radians (more than 0, at most a whole turn) about an axis
/// leaves its end within OCCT's length tolerance of its start, at the profile's farthest
/// point, `farthest` from the axis: that point travels `farthest·angle` along its circle and
/// stops `farthest·(2π − angle)` short of where it began. Points within the tolerance are
/// one place, so either way the turn's ends are one: it moves nothing, or it's short of a
/// whole turn by nothing OCCT can tell apart, unless it's a whole turn ([`is_whole_turn`]),
/// which has no ends. FreeCAD checks only the angle, above `Precision::Angular()` and at
/// most 360° (docs/ports/extrude-revolve.md). The rule for M1's revolve and for a sketch's
/// revolve and groove alike.
pub fn end_at_start(farthest: f64, angle: f64) -> Option<EndAtStart> {
    if farthest * angle <= LENGTH_TOLERANCE {
        Some(EndAtStart::Unmoved)
    } else if !is_whole_turn(angle) && farthest * (TAU - angle) <= LENGTH_TOLERANCE {
        Some(EndAtStart::ShortOfWhole)
    } else {
        None
    }
}

/// What a sweep is, for the history it must report ([`History::sweep_fault`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepKind {
    /// A prism, through all or not.
    Prism,
    /// A revolution of less than a whole turn.
    Turn,
    /// A revolution of a whole turn ([`is_whole_turn`]).
    WholeTurn,
}

impl SweepKind {
    /// The sweep a revolution of `angle` radians is.
    pub fn of_turn(angle: f64) -> Self {
        if is_whole_turn(angle) {
            SweepKind::WholeTurn
        } else {
            SweepKind::Turn
        }
    }
}

/// The kind of element a profile element of `kind` generates as its side: a face its solid,
/// an edge a face, a vertex an edge.
fn side_kind(kind: ElementKind) -> ElementKind {
    match kind {
        ElementKind::Face => ElementKind::Solid,
        ElementKind::Edge => ElementKind::Face,
        ElementKind::Vertex | ElementKind::Solid => ElementKind::Edge,
    }
}

impl History {
    /// What's wrong with a sweep's history (already checked by [`History::validate`] for its
    /// one input, the faces), if anything, by design S3's and S4's rule. Each face, edge, and
    /// vertex of the faces:
    ///
    /// - is never kept: its start counterpart (OCCT's `FirstShape`, the element itself or its
    ///   moved copy) is its one `modified` element;
    /// - generates its end counterpart (OCCT's `LastShape`), of its own kind, unless that's
    ///   its start (an edge or a vertex on a revolution's axis);
    /// - generates its sides, of the next kind up: a face its solid (exactly one), an edge a
    ///   face, a vertex an edge; and nothing else.
    ///
    /// A prism gives every element a start, an end, and one side, so its result has exactly
    /// those: twice the faces' faces and the edges' side faces; twice the edges and the
    /// vertices' side edges; twice the vertices. A revolution of less than a whole turn gives
    /// every element a start, and every face its end and its solid; an element on the axis
    /// has no end apart from its start, and sweeps nothing. A whole turn gives nothing an end,
    /// and its face no start: OCCT reports the face, and the edges that sweep flat faces
    /// (square to the axis), nothing in the result; an edge that sweeps a face of revolution
    /// starts at that face's seam, and a vertex off the axis is its circle's vertex
    /// (docs/ports/history.md). `ends` are for M1's polygon sweeps, and are empty here. No
    /// result element is the counterpart or side of two elements, and the result has a
    /// solid for each face.
    pub fn sweep_fault(&self, kind: SweepKind) -> Option<String> {
        let input = self.inputs.first()?;
        if !self.ends.start.is_empty() || !self.ends.end.is_empty() {
            return Some(
                "the sweep reports whole-shape ends, but a sweep of faces reports each element's"
                    .to_owned(),
            );
        }
        if self.result.solids as usize != input.faces.len() {
            return Some(format!(
                "the sweep's result has {} solids for {} faces",
                self.result.solids,
                input.faces.len()
            ));
        }
        let mut claimed = BTreeSet::new();
        for (element_kind, fates) in kinds(input) {
            for (n, fate) in fates.iter().enumerate() {
                let what = || format!("{element_kind:?}{}", n + 1);
                if let Some(kept) = fate.kept {
                    return Some(format!(
                        "{} is kept as {element_kind:?}{kept}: a sweep's start is modified, never kept",
                        what()
                    ));
                }
                let starts = fate.modified.len();
                let ends: Vec<_> = fate
                    .generated
                    .iter()
                    .filter(|r| r.element.kind == element_kind)
                    .collect();
                let sides = fate
                    .generated
                    .iter()
                    .filter(|r| r.element.kind == side_kind(element_kind))
                    .count();
                if ends.len() + sides != fate.generated.len() {
                    return Some(format!(
                        "{} generates an element neither its end nor its side",
                        what()
                    ));
                }
                let (want_starts, want_ends, want_sides) = match (kind, element_kind) {
                    (SweepKind::Prism, _) => (1..=1, 1..=1, 1..=1),
                    (SweepKind::Turn, ElementKind::Face) => (1..=1, 1..=1, 1..=1),
                    (SweepKind::Turn, _) => (1..=1, 0..=1, 0..=usize::MAX),
                    (SweepKind::WholeTurn, ElementKind::Face) => (0..=0, 0..=0, 0..=1),
                    (SweepKind::WholeTurn, _) => (0..=1, 0..=0, 0..=usize::MAX),
                };
                for (count, want, name) in [
                    (starts, want_starts, "start"),
                    (ends.len(), want_ends, "end"),
                    (sides, want_sides, "side"),
                ] {
                    if !want.contains(&count) {
                        return Some(format!(
                            "{} has {count} {name} counterparts in a {kind:?} sweep",
                            what()
                        ));
                    }
                }
                if kind == SweepKind::Turn
                    && let Some(fault) = self.turn_fault(element_kind, fate, &ends, sides)
                {
                    return Some(format!("{} {fault}", what()));
                }
                for r in fate.modified.iter().chain(&fate.generated) {
                    if !claimed.insert(r.element) {
                        return Some(format!(
                            "{} is reported for {} and for another element",
                            r.element,
                            what()
                        ));
                    }
                }
            }
        }
        if kind == SweepKind::Prism {
            let (f, e, v) = (
                input.faces.len() as u64,
                input.edges.len() as u64,
                input.vertices.len() as u64,
            );
            let want = (2 * f + e, 2 * e + v, 2 * v);
            let got = (
                u64::from(self.result.faces),
                u64::from(self.result.edges),
                u64::from(self.result.vertices),
            );
            if got != want {
                return Some(format!(
                    "the prism has {got:?} faces, edges, and vertices, but its faces make {want:?}"
                ));
            }
        }
        None
    }
}

impl History {
    /// What's wrong with how a turn short of a whole one reports an edge's or a vertex's
    /// fate, by what OCCT does (`crates/silico-kernel/tests/sweep_faces.rs`,
    /// `adversarial_sweep_faces.rs`):
    ///
    /// - An edge off the axis has its end and one side face; one along the axis has neither
    ///   (its start is its end, and it sweeps nothing).
    /// - A vertex off the axis has its end and one side edge, an arc from its start to its
    ///   end. A vertex on the axis has no end (its start is its end), and as sides only the
    ///   degenerate edges OCCT generates where a face of revolution closes on the axis (a
    ///   cone's apex): each an edge whose one vertex is the vertex's start, none where no
    ///   face closes there, and two where `MakeRevol` copies one that two faces share (a
    ///   diamond's tip).
    fn turn_fault(
        &self,
        kind: ElementKind,
        fate: &crate::ElementFate,
        ends: &[&crate::Reported],
        sides: usize,
    ) -> Option<String> {
        let start = fate.modified.first()?.element.index;
        let vertices = |edge: u32| -> Option<&Vec<u32>> {
            self.topology
                .edge_vertices
                .get((edge as usize).checked_sub(1)?)
        };
        let side_edges = || {
            fate.generated
                .iter()
                .filter(|r| r.element.kind == ElementKind::Edge)
                .map(|r| r.element.index)
        };
        match (kind, ends.first()) {
            (ElementKind::Edge, Some(_)) if sides == 1 => None,
            (ElementKind::Edge, None) if sides == 0 => None,
            (ElementKind::Edge, _) => Some(format!(
                "has {} end and {sides} side counterparts in a Turn sweep: an edge on the axis has neither, and one off it both",
                ends.len()
            )),
            (ElementKind::Vertex, Some(end)) => {
                let arc = side_edges().next().and_then(vertices).map(|v| {
                    let mut v = v.clone();
                    v.sort_unstable();
                    v
                });
                let mut want = vec![start, end.element.index];
                want.sort_unstable();
                (sides != 1 || arc.as_ref() != Some(&want)).then(|| {
                    format!(
                        "has its end and {sides} side edges in a Turn sweep, {arc:?}: a vertex off the axis sweeps one arc from its start to its end"
                    )
                })
            }
            (ElementKind::Vertex, None) => side_edges()
                .find(|&edge| vertices(edge).map(Vec::as_slice) != Some(&[start]))
                .map(|edge| {
                    format!(
                        "has no end, but sweeps Edge{edge}, which isn't a point at its start: a vertex on the axis sweeps only degenerate edges"
                    )
                }),
            _ => None,
        }
    }
}

/// An input's fates, kind by kind.
fn kinds(input: &InputHistory) -> [(ElementKind, &[crate::ElementFate]); 3] {
    [
        (ElementKind::Face, &input.faces),
        (ElementKind::Edge, &input.edges),
        (ElementKind::Vertex, &input.vertices),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ElementCounts, ElementFate, ElementRef, Reported};

    fn r(kind: ElementKind, index: u32) -> Reported {
        Reported {
            element: ElementRef { kind, index },
            position: 1,
        }
    }

    /// A prism's history of one triangle: Face1, Edge1–3, Vertex1–3. The result numbers its
    /// side faces 1–3, the start face 4, the end 5; the start edges 1–3, the end edges 4–6,
    /// the side edges 7–9; the start vertices 1–3, the end 4–6; its solid 1.
    fn triangle_prism() -> History {
        let fate = |modified: Reported, generated: Vec<Reported>| ElementFate {
            kept: None,
            modified: vec![modified],
            generated,
        };
        use ElementKind::{Edge, Face, Solid, Vertex};
        History {
            inputs: vec![InputHistory {
                faces: vec![fate(r(Face, 4), vec![r(Face, 5), r(Solid, 1)])],
                edges: (1..=3)
                    .map(|i| fate(r(Edge, i), vec![r(Edge, i + 3), r(Face, i)]))
                    .collect(),
                vertices: (1..=3)
                    .map(|i| fate(r(Vertex, i), vec![r(Vertex, i + 3), r(Edge, i + 6)]))
                    .collect(),
            }],
            result: ElementCounts {
                faces: 5,
                edges: 9,
                vertices: 6,
                solids: 1,
            },
            // Each start edge from its vertex to the next, each end edge likewise, and each
            // side edge from a start vertex to its end.
            topology: crate::Topology {
                edge_vertices: (0..3)
                    .map(|i| vec![i + 1, (i + 1) % 3 + 1])
                    .chain((0..3).map(|i| vec![i + 4, (i + 1) % 3 + 4]))
                    .chain((1..=3).map(|j| vec![j, j + 3]))
                    .collect(),
                ..crate::Topology::default()
            },
            ..History::default()
        }
    }

    #[test]
    fn a_prism_that_keeps_the_rule_passes_and_one_that_breaks_it_is_refused_saying_why() {
        let good = triangle_prism();
        assert_eq!(good.sweep_fault(SweepKind::Prism), None);
        // A turn may report the same; so may a whole turn without its ends.
        assert_eq!(good.sweep_fault(SweepKind::Turn), None);
        type Spoil = fn(&mut History);
        let cases: [(Spoil, &str); 11] = [
            (
                |h| h.inputs[0].edges[1].kept = Some(2),
                "Edge2 is kept as Edge2: a sweep's start is modified, never kept",
            ),
            (
                |h| h.inputs[0].vertices[0].modified.clear(),
                "Vertex1 has 0 start counterparts in a Prism sweep",
            ),
            (
                |h| {
                    h.inputs[0].edges[2]
                        .generated
                        .retain(|r| r.element.kind != ElementKind::Edge)
                },
                "Edge3 has 0 end counterparts in a Prism sweep",
            ),
            (
                |h| {
                    h.inputs[0].faces[0]
                        .generated
                        .retain(|r| r.element.kind != ElementKind::Solid)
                },
                "Face1 has 0 side counterparts in a Prism sweep",
            ),
            (
                |h| {
                    h.inputs[0].vertices[2]
                        .generated
                        .push(r(ElementKind::Face, 3))
                },
                "Vertex3 generates an element neither its end nor its side",
            ),
            (
                |h| h.inputs[0].edges[0].generated[0] = r(ElementKind::Edge, 5),
                "Edge5 is reported for Edge2 and for another element",
            ),
            (
                |h| h.ends.start = vec![4],
                "the sweep reports whole-shape ends",
            ),
            (
                |h| h.result.solids = 2,
                "the sweep's result has 2 solids for 1 faces",
            ),
            (
                |h| h.result.faces = 6,
                "the prism has (6, 9, 6) faces, edges, and vertices, but its faces make (5, 9, 6)",
            ),
            // The end its start: only an element on a revolution's axis may lose its end.
            (
                |h| h.inputs[0].vertices[1].generated[0] = r(ElementKind::Vertex, 2),
                "Vertex2 is reported for Vertex2 and for another element",
            ),
            (
                |h| h.inputs[0].faces[0].modified.push(r(ElementKind::Face, 3)),
                "Face1 has 2 start counterparts in a Prism sweep",
            ),
        ];
        for (spoil, why) in cases {
            let mut bad = good.clone();
            spoil(&mut bad);
            let fault = bad.sweep_fault(SweepKind::Prism).unwrap_or_default();
            assert!(fault.starts_with(why), "{fault} (want {why})");
        }
    }

    #[test]
    fn a_turn_tells_an_axis_element_by_what_occt_reports_and_a_whole_turn_has_no_ends() {
        let mut on_axis = triangle_prism();
        // Edge1 and its two vertices on the axis: their ends are their starts, unreported,
        // and they sweep nothing.
        on_axis.inputs[0].edges[0].generated.clear();
        on_axis.inputs[0].vertices[0].generated.clear();
        on_axis.inputs[0].vertices[1].generated.clear();
        assert_eq!(on_axis.sweep_fault(SweepKind::Turn), None);
        assert!(on_axis.sweep_fault(SweepKind::Prism).is_some());
        // An end without a side, or a side without an end, is neither on the axis nor off.
        let mut half = on_axis.clone();
        half.inputs[0].edges[1].generated.remove(0);
        assert_eq!(
            half.sweep_fault(SweepKind::Turn).as_deref(),
            Some(
                "Edge2 has 0 end and 1 side counterparts in a Turn sweep: an edge on the axis has neither, and one off it both"
            )
        );
        let mut half = on_axis.clone();
        half.inputs[0].vertices[2].generated.pop();
        assert!(
            half.sweep_fault(SweepKind::Turn)
                .unwrap()
                .starts_with("Vertex3 has its end and 0 side edges in a Turn sweep")
        );
        // A vertex on the axis where a face of revolution closes (a cone's apex) sweeps a
        // degenerate edge, a point at its start; where two faces close, two (MakeRevol copies
        // the one they'd share).
        let mut apex = on_axis.clone();
        apex.inputs[0].vertices[0].generated = vec![r(ElementKind::Edge, 7)];
        apex.topology.edge_vertices[6] = vec![1];
        assert_eq!(apex.sweep_fault(SweepKind::Turn), None);
        apex.inputs[0].vertices[1].generated = vec![r(ElementKind::Edge, 8)];
        apex.topology.edge_vertices[7] = vec![2];
        assert_eq!(apex.sweep_fault(SweepKind::Turn), None);
        // A point elsewhere, or an edge with two vertices, isn't the vertex's apex.
        let mut elsewhere = apex.clone();
        elsewhere.topology.edge_vertices[6] = vec![3];
        assert_eq!(
            elsewhere.sweep_fault(SweepKind::Turn).as_deref(),
            Some(
                "Vertex1 has no end, but sweeps Edge7, which isn't a point at its start: a vertex on the axis sweeps only degenerate edges"
            )
        );
        let mut long = apex.clone();
        long.topology.edge_vertices[6] = vec![1, 4];
        assert!(
            long.sweep_fault(SweepKind::Turn)
                .unwrap()
                .starts_with("Vertex1 has no end, but sweeps Edge7")
        );
        // A vertex off the axis sweeps an arc from its start to its end, and one only.
        let mut astray = triangle_prism();
        astray.topology.edge_vertices[8] = vec![3, 5];
        assert!(
            astray
                .sweep_fault(SweepKind::Turn)
                .unwrap()
                .starts_with("Vertex3 has its end and 1 side edges in a Turn sweep, Some([3, 5])")
        );
        // The prism's rule doesn't read the topology.
        assert_eq!(astray.sweep_fault(SweepKind::Prism), None);
        // But every element has its start, and the face its end and solid.
        let mut no_start = on_axis.clone();
        no_start.inputs[0].edges[0].modified.clear();
        assert_eq!(
            no_start.sweep_fault(SweepKind::Turn).as_deref(),
            Some("Edge1 has 0 start counterparts in a Turn sweep")
        );
        let mut no_end = on_axis.clone();
        no_end.inputs[0].faces[0].generated.remove(0);
        assert_eq!(
            no_end.sweep_fault(SweepKind::Turn).as_deref(),
            Some("Face1 has 0 end counterparts in a Turn sweep")
        );
        // A whole turn: no ends at all; starts where OCCT has them.
        let mut whole = triangle_prism();
        let input = &mut whole.inputs[0];
        for fate in input
            .faces
            .iter_mut()
            .chain(&mut input.edges)
            .chain(&mut input.vertices)
        {
            let kind = fate.modified[0].element.kind;
            fate.generated.retain(|r| r.element.kind != kind);
        }
        // As OCCT reports it: the face nothing; each edge and vertex its seam's start.
        input.faces[0] = ElementFate::default();
        assert_eq!(whole.sweep_fault(SweepKind::WholeTurn), None);
        let mut face_start = whole.clone();
        face_start.inputs[0].faces[0].modified = vec![r(ElementKind::Face, 4)];
        assert_eq!(
            face_start.sweep_fault(SweepKind::WholeTurn).as_deref(),
            Some("Face1 has 1 start counterparts in a WholeTurn sweep")
        );
        let mut edge_end = whole.clone();
        edge_end.inputs[0].edges[1]
            .generated
            .insert(0, r(ElementKind::Edge, 5));
        assert_eq!(
            edge_end.sweep_fault(SweepKind::WholeTurn).as_deref(),
            Some("Edge2 has 1 end counterparts in a WholeTurn sweep")
        );
    }

    #[test]
    fn values_a_sweep_cant_take_are_refused_naming_them() {
        assert_eq!(check_prism([0.0, 0.0, -5.0], [0.0, 0.0, 10.0]), Ok(()));
        assert_eq!(check_prism([1e6, -1e6, 0.0], [0.0, -1e6, 0.0]), Ok(()));
        for start in [
            [f64::NAN, 0.0, 0.0],
            [0.0, 1.0000001e6, 0.0],
            [0.0, 0.0, f64::INFINITY],
        ] {
            assert_eq!(
                check_prism(start, [0.0, 0.0, 1.0]).unwrap_err().parameter,
                "start"
            );
        }
        for vector in [
            [0.0; 3],
            [0.0, 0.0, 1e-7],
            [0.0, 0.0, 1.0000001e6],
            [f64::NAN, 0.0, 1.0],
            [7.1e5, 7.1e5, 0.0],
        ] {
            assert_eq!(
                check_prism([0.0; 3], vector).unwrap_err().parameter,
                "vector",
                "{vector:?}"
            );
        }
        assert_eq!(check_prism([0.0; 3], [0.0, 0.0, 1.01e-7]), Ok(()));
        // Directions: a unit vector, to rounding.
        assert_eq!(check_through_all([0.0, -1.0, 0.0]), Ok(()));
        let s = std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(check_through_all([s, s, 0.0]), Ok(()));
        for direction in [
            [0.0, 0.0, 2.0],
            [0.0, 0.0, 1.0 + 1e-12],
            [0.0; 3],
            [f64::NAN, 1.0, 0.0],
        ] {
            assert_eq!(
                check_through_all(direction).unwrap_err().parameter,
                "direction",
                "{direction:?}"
            );
        }
        let z = [0.0, 0.0, 1.0];
        assert_eq!(check_revolution([0.0; 3], z, 0.0, TAU), Ok(()));
        assert_eq!(
            check_revolution([0.0; 3], z, -std::f64::consts::PI, TAU),
            Ok(())
        );
        assert_eq!(check_revolution([3.0, 0.0, 1e6], z, 0.0, 1.1e-12), Ok(()));
        for (origin, direction, start, angle, parameter) in [
            ([f64::NAN, 0.0, 0.0], z, 0.0, 1.0, "axis"),
            ([0.0, 2e6, 0.0], z, 0.0, 1.0, "axis"),
            ([0.0; 3], [0.0, 0.0, 0.5], 0.0, 1.0, "axis"),
            ([0.0; 3], z, f64::NAN, 1.0, "start"),
            ([0.0; 3], z, -TAU - 1e-9, 1.0, "start"),
            ([0.0; 3], z, 0.0, 0.0, "angle"),
            ([0.0; 3], z, 0.0, 1e-12, "angle"),
            ([0.0; 3], z, 0.0, -1.0, "angle"),
            ([0.0; 3], z, 0.0, TAU + 1e-9, "angle"),
            ([0.0; 3], z, 0.0, f64::NAN, "angle"),
        ] {
            assert_eq!(
                check_revolution(origin, direction, start, angle)
                    .unwrap_err()
                    .parameter,
                parameter
            );
        }
    }

    #[test]
    fn a_sweep_must_leave_the_faces_plane_and_a_turn_keep_its_axis_in_it() {
        let z = [0.0, 0.0, 1.0];
        // Along the normal, either way, or leaning: out of the plane.
        for vector in [[0.0, 0.0, 10.0], [0.0, 0.0, -1e-6], [5.0, 0.0, 1e-6]] {
            assert_eq!(
                check_leaves_plane("vector", z, vector),
                Ok(()),
                "{vector:?}"
            );
        }
        // FreeCAD's 1e-7 of the vector's length along the normal: a hair over, out; a hair
        // under, in.
        assert_eq!(check_leaves_plane("vector", z, [1.0, 0.0, 1.01e-7]), Ok(()));
        for vector in [
            [1.0, 0.0, 0.99e-7],
            [5.0, 0.0, 0.0],
            [0.0, -3.0, -0.0],
            [0.0; 3],
        ] {
            let e = check_leaves_plane("direction", z, vector).unwrap_err();
            assert_eq!(e.parameter, "direction", "{vector:?}");
            assert!(
                e.message.contains("lies in the faces' plane"),
                "{}",
                e.message
            );
        }
        // An axis in the plane, or leaning by at most 1e-7: in; more, or along the normal:
        // out.
        for direction in [
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.6, 0.8, 0.0],
            [1.0, 0.0, 1e-7],
        ] {
            assert_eq!(check_axis_in_plane(z, direction), Ok(()), "{direction:?}");
        }
        for direction in [z, [0.0, 0.0, -1.0], [1.0, 0.0, 1.01e-7], [0.6, 0.0, 0.8]] {
            assert_eq!(
                check_axis_in_plane(z, direction).unwrap_err().parameter,
                "axis"
            );
        }
    }

    #[test]
    fn every_component_of_the_normal_counts_in_the_plane_checks() {
        // A normal and a vector along it with two components alike, s = 1/√2: the dot
        // product is s² + s² = 1 (to rounding), its terms equal, so a check that took one
        // term from another would read 0.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        for normal in [[s, s, 0.0], [0.0, s, s]] {
            assert_eq!(
                check_leaves_plane("vector", normal, normal),
                Ok(()),
                "{normal:?}"
            );
            assert_eq!(
                check_axis_in_plane(normal, normal).unwrap_err().parameter,
                "axis",
                "{normal:?}"
            );
        }
        // An axis along an x normal (the YZ plane's) leans out of it: 1 · 1 + 0 + 0 = 1.
        let x = [1.0, 0.0, 0.0];
        assert_eq!(check_axis_in_plane(x, x).unwrap_err().parameter, "axis");
        assert_eq!(check_axis_in_plane(x, [0.0, 0.6, 0.8]), Ok(()));
    }

    #[test]
    fn a_through_all_of_exactly_the_tolerance_is_refused() {
        // 1e-7 / 2.02 rounds to 4.95049504950495e-8, which 2.02 times rounds back to
        // exactly 1e-7 (worked in doubles: the product's nearest double is 1e-7's); one ulp
        // more gives 1.0000000000000001e-7. A sweep must be longer than 1e-7: the first is
        // refused, the second taken.
        let d = 4.95049504950495e-8;
        assert_eq!(THROUGH_ALL_FACTOR * d, 1e-7);
        let refused = through_all_length([[0.0; 3], [d, 0.0, 0.0]]).unwrap_err();
        assert_eq!(refused.parameter, "base");
        let next = f64::from_bits(d.to_bits() + 1);
        assert_eq!(
            through_all_length([[0.0; 3], [next, 0.0, 0.0]]),
            Ok(1.0000000000000001e-7)
        );
    }

    #[test]
    fn an_edge_off_a_turns_axis_sweeps_one_side_face_no_more_no_fewer() {
        // An edge with its end has left the axis, so it sweeps exactly one face: none, or
        // two, is a fault.
        let good = triangle_prism();
        assert_eq!(good.sweep_fault(SweepKind::Turn), None);
        let mut none = good.clone();
        none.inputs[0].edges[1]
            .generated
            .retain(|r| r.element.kind != ElementKind::Face);
        assert_eq!(
            none.sweep_fault(SweepKind::Turn).as_deref(),
            Some(
                "Edge2 has 1 end and 0 side counterparts in a Turn sweep: an edge on the axis has neither, and one off it both"
            )
        );
        let mut two = good.clone();
        two.inputs[0].edges[1]
            .generated
            .push(r(ElementKind::Face, 3));
        assert_eq!(
            two.sweep_fault(SweepKind::Turn).as_deref(),
            Some(
                "Edge2 has 1 end and 2 side counterparts in a Turn sweep: an edge on the axis has neither, and one off it both"
            )
        );
    }

    #[test]
    fn a_whole_turn_is_360_degrees_as_freecad_converts_them_and_within_occts_tolerance() {
        assert_eq!(crate::radians(360.0), TAU);
        assert!(is_whole_turn(crate::radians(360.0)));
        assert!(is_whole_turn(TAU - 0.9e-12));
        assert!(is_whole_turn(TAU + 0.9e-12));
        assert!(!is_whole_turn(TAU - 2e-12));
        // A nanodegree short of a turn is a turn short of a whole one.
        assert!(!is_whole_turn(crate::radians(360.0 - 1e-9)));
        assert_eq!(SweepKind::of_turn(TAU), SweepKind::WholeTurn);
        assert_eq!(SweepKind::of_turn(crate::radians(90.0)), SweepKind::Turn);
    }

    #[test]
    fn a_turn_whose_end_is_within_the_tolerance_of_its_start_either_way_is_found() {
        // A farthest point 10 mm off the axis: 1e-7 mm of its circle is 1e-8 radians, or
        // 1e-8 · 180/π = 5.7296e-7 degrees, 572.96 nanodegrees. So 360° less 572
        // nanodegrees leaves its ends 9.983e-8 mm apart, one place; less 573, 1.00007e-7.
        let short = |ndeg: f64| crate::radians(360.0 - ndeg * 1e-9);
        assert_eq!(
            end_at_start(10.0, short(572.0)),
            Some(EndAtStart::ShortOfWhole)
        );
        assert_eq!(
            end_at_start(10.0, short(1.0)),
            Some(EndAtStart::ShortOfWhole)
        );
        assert_eq!(end_at_start(10.0, short(573.0)), None);
        // Within OCCT's angular tolerance of a turn it's a whole turn, with no ends.
        assert_eq!(end_at_start(10.0, TAU), None);
        assert_eq!(end_at_start(10.0, TAU - 0.9e-12), None);
        assert_eq!(
            end_at_start(10.0, TAU - 2e-12),
            Some(EndAtStart::ShortOfWhole)
        );
        // A kilometre off, 2e-12 radians short leaves its ends 2e-6 mm apart.
        assert_eq!(end_at_start(1e6, TAU - 2e-12), None);
        // The near-0 side: 572 nanodegrees moves it 9.983e-8 mm, 573 1.00007e-7.
        assert_eq!(
            end_at_start(10.0, crate::radians(572e-9)),
            Some(EndAtStart::Unmoved)
        );
        assert_eq!(end_at_start(10.0, crate::radians(573e-9)), None);
        // A profile so near its axis that a whole turn moves it no more than the tolerance
        // moves nothing.
        assert_eq!(end_at_start(1e-8, TAU), Some(EndAtStart::Unmoved));
    }

    #[test]
    fn through_all_is_2_02_times_the_diagonal_and_refused_past_a_kilometre() {
        // A 30 × 40 × 0 box: diagonal 50, so 101.
        assert_eq!(through_all_length([[0.0; 3], [30.0, 40.0, 0.0]]), Ok(101.0));
        // A 10 mm cube and a profile on its top: √300 × 2.02.
        let cube = through_all_length([[0.0; 3], [10.0; 3]]).unwrap();
        assert!((cube - 2.02 * 300f64.sqrt()).abs() < 1e-12, "{cube}");
        // 495 049.5 across is 999 999.99: allowed; 495 050 is 1 000 001: refused.
        assert!(through_all_length([[0.0; 3], [495_049.5, 0.0, 0.0]]).is_ok());
        let far = through_all_length([[0.0; 3], [495_050.0, 0.0, 0.0]]).unwrap_err();
        assert_eq!(far.parameter, "base");
        assert!(
            far.message.contains("495050 mm across, is 1000001"),
            "{}",
            far.message
        );
        assert_eq!(
            through_all_length([[1.0; 3], [1.0; 3]])
                .unwrap_err()
                .parameter,
            "base"
        );
        assert_eq!(
            through_all_length([[0.0; 3], [f64::NAN, 1.0, 1.0]])
                .unwrap_err()
                .parameter,
            "base"
        );
    }
}
