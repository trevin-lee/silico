//! The kernel worker (ARCHITECTURE.md, Process model): the worker loop every worker shares
//! (`silico_worker`), serving OCCT's operations. Workers keep no state: shapes arrive and
//! leave as B-rep.
//!
//! [`run`] is the whole worker; the production binary passes it [`handle`], and the test
//! worker passes a handler that adds the test operations.

use std::io::Write;
use std::process::ExitCode;

use silico_protocol::{
    Blob, BuiltData, Description, Failure, FreecadMeasures, Generation, Op, Refusal, Reply,
    RequestId, ShapeData, ShapeSummary,
};

use crate::KernelError;

/// The production operations. Test operations get [`Refusal::TestOperation`].
pub fn handle(op: Op) -> Result<Reply, Failure> {
    match op {
        Op::MakePrimitive(primitive) => {
            let shape = crate::make(&primitive).map_err(failure)?;
            shape_reply(&shape)
        }
        Op::Summarize { shape } => {
            let shape = shape_of(&shape)?;
            let summary = crate::summarize(&shape).map_err(failure)?;
            check_finite(&summary)?;
            Ok(Reply::Summary(summary))
        }
        Op::MeasureAsFreecad { shape } => {
            let shape = shape_of(&shape)?;
            let measures = crate::measure_as_freecad(&shape).map_err(failure)?;
            check_freecad_finite(&measures)?;
            Ok(Reply::FreecadMeasures(measures))
        }
        Op::Describe {
            shape,
            kind,
            first,
            count,
        } => {
            let shape = shape_of(&shape)?;
            let (total, description) = crate::describe(
                &shape,
                kind,
                first,
                count.min(silico_protocol::DESCRIBE_BATCH),
            )
            .map_err(failure)?;
            described(total, description)
        }
        Op::Fillet {
            shape,
            edges,
            radius,
        } => {
            let shape = shape_of(&shape)?;
            let rounded = crate::fillet(&shape, &edges, radius).map_err(failure)?;
            built_reply(rounded)
        }
        Op::Chamfer {
            shape,
            edges,
            size,
            flip,
        } => {
            let shape = shape_of(&shape)?;
            built_reply(crate::chamfer(&shape, &edges, size, flip).map_err(failure)?)
        }
        Op::Translate { shape, offset } => {
            let shape = shape_of(&shape)?;
            built_reply(crate::translate(&shape, offset).map_err(failure)?)
        }
        Op::Extrude {
            profile,
            length,
            direction,
        } => built_reply(crate::extrude(&profile, length, direction).map_err(failure)?),
        Op::Revolve {
            profile,
            axis,
            angle,
        } => built_reply(crate::revolve(&profile, &axis, angle).map_err(failure)?),
        Op::MakeFaces(regions) => built_reply(crate::make_faces(&regions).map_err(failure)?),
        Op::Prism {
            faces,
            start,
            vector,
        } => {
            let faces = shape_of(&faces)?;
            built_reply(crate::prism(&faces, start, vector).map_err(failure)?)
        }
        Op::PrismThroughAll {
            faces,
            base,
            direction,
        } => {
            let faces = shape_of(&faces)?;
            let base = shape_of(&base)?;
            built_reply(crate::prism_through_all(&faces, &base, direction).map_err(failure)?)
        }
        Op::Revolution {
            faces,
            axis,
            start,
            angle,
        } => {
            let faces = shape_of(&faces)?;
            built_reply(crate::revolution(&faces, &axis, start, angle).map_err(failure)?)
        }
        Op::Boolean { op, base, tool } => {
            let base = shape_of(&base)?;
            let tool = shape_of(&tool)?;
            built_reply(crate::boolean(op, &base, &tool).map_err(failure)?)
        }
        Op::Tessellate { shape, deflection } => {
            let shape = shape_of(&shape)?;
            let mesh = crate::tessellate(&shape, deflection).map_err(failure)?;
            Ok(Reply::Mesh(mesh))
        }
        Op::Echo { data } => Ok(Reply::Echo { data }),
        Op::Solve(_) | Op::DragBegin(_) | Op::DragMove { .. } | Op::DragEnd => {
            Err(Failure::new(Refusal::OtherWorkersOp {
                op: op.name().to_owned(),
            }))
        }
        Op::Debug(_) => Err(Failure::new(Refusal::TestOperation {
            op: op.name().to_owned(),
        })),
    }
}

/// A shape from a request, with the validity it came with, if a worker checked it (if
/// not, it's checked when it's first needed).
fn shape_of(data: &ShapeData) -> Result<crate::Shape, Failure> {
    let shape = crate::from_brep(data.brep().as_bytes()).map_err(failure)?;
    Ok(match data.valid() {
        Some(valid) => shape.with_validity(valid),
        None => shape,
    })
}

/// A shape to send back, with its validity (checked now if it wasn't already).
fn shape_data(shape: &crate::Shape) -> Result<ShapeData, Failure> {
    Ok(ShapeData::checked(
        Blob::inline(crate::to_brep(shape).map_err(failure)?),
        shape.is_valid().map_err(failure)?,
    ))
}

fn shape_reply(shape: &crate::Shape) -> Result<Reply, Failure> {
    Ok(Reply::Shape(shape_data(shape)?))
}

/// An operation's result and its history.
fn built_reply(built: crate::Built) -> Result<Reply, Failure> {
    Ok(Reply::Built(BuiltData {
        shape: shape_data(&built.shape)?,
        history: built.history,
    }))
}

/// A shape OCCT read but can't integrate gives NaN or infinity: never a result.
fn check_finite(summary: &ShapeSummary) -> Result<(), Failure> {
    measures_finite(summary.volume, summary.area, summary.center, summary.bounds)
}

/// As [`check_finite`], for a shape measured as FreeCAD measures it.
/// A batch of a description as the worker sends it: refused, naming the value, if one isn't
/// finite (the core's check, `Description::validate`, names a value the same way).
fn described(total: u32, description: Description) -> Result<Reply, Failure> {
    match description.non_finite() {
        Some((quantity, value)) => Err(Failure::new(Refusal::NotFinite {
            quantity: quantity.to_string(),
            value,
        })),
        None => Ok(Reply::Described { total, description }),
    }
}

fn check_freecad_finite(m: &FreecadMeasures) -> Result<(), Failure> {
    measures_finite(m.volume, m.area, m.center, m.bounds)
}

/// Whether a shape's measures are all finite, naming the first that isn't.
fn measures_finite(
    volume: f64,
    area: f64,
    [x, y, z]: [f64; 3],
    bounds: Option<[[f64; 3]; 2]>,
) -> Result<(), Failure> {
    let [low, high] = bounds.unwrap_or_default();
    all_finite([
        ("volume", volume),
        ("area", area),
        ("center x", x),
        ("center y", y),
        ("center z", z),
        ("bounding box's lowest x", low[0]),
        ("bounding box's lowest y", low[1]),
        ("bounding box's lowest z", low[2]),
        ("bounding box's highest x", high[0]),
        ("bounding box's highest y", high[1]),
        ("bounding box's highest z", high[2]),
    ])
}

fn all_finite<const N: usize>(values: [(&str, f64); N]) -> Result<(), Failure> {
    match values.into_iter().find(|(_, value)| !value.is_finite()) {
        None => Ok(()),
        Some((quantity, value)) => Err(Failure::new(Refusal::NotFinite {
            quantity: quantity.to_owned(),
            value,
        })),
    }
}

/// What the worker sends for `e`: why, as data, and OCCT's words.
fn failure(e: KernelError) -> Failure {
    Failure {
        refusal: e.refusal().clone(),
        cause: e.cause().map(str::to_owned),
    }
}

/// How the kernel worker reads and switches off floating-point traps: through OCCT.
pub const PLATFORM: silico_worker::Platform = silico_worker::Platform {
    fp_trap_mask: crate::fp_trap_mask,
    disable_fp_traps,
    built_from: crate::OCCT_BUILD,
};

fn disable_fp_traps() -> Result<(), String> {
    crate::disable_fp_traps().map_err(|e| e.message().to_owned())
}

/// Fails if any floating-point trap is on. Checked at startup and after every request.
pub fn check_fp_traps() -> Result<(), String> {
    silico_worker::check_fp_traps(&PLATFORM)
}

pub use silico_worker::check_fp_trap_mask;

/// The kernel worker process (see [`silico_worker::run`]).
pub fn run<F>(handle: F) -> ExitCode
where
    F: FnMut(RequestId, Generation, Op, &mut dyn Write) -> Result<Reply, Failure>,
{
    silico_worker::run(&PLATFORM, handle)
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_description_with_a_value_not_finite_is_refused_naming_it() {
        use super::{Refusal, Reply, described};
        use silico_protocol::{Description, EdgeCurve, EdgeDescription};
        let mut d = Description {
            faces: Vec::new(),
            edges: vec![EdgeDescription {
                curve: EdgeCurve::Line,
                length: 1.0,
                start: [0.0; 3],
                end: [1.0, 0.0, 0.0],
                middle: [0.5, f64::NAN, 0.0],
            }],
            vertices: Vec::new(),
        };
        match described(1, d.clone()).unwrap_err().refusal {
            Refusal::NotFinite { quantity, value } => {
                assert_eq!(quantity, "the kernel's Edge1's middle");
                assert!(value.is_nan());
            }
            other => panic!("{other:?}"),
        }
        d.edges[0].middle[1] = 0.0;
        assert!(matches!(
            described(1, d),
            Ok(Reply::Described { total: 1, .. })
        ));
    }

    #[test]
    fn a_kernel_error_reaches_the_app_as_its_refusal_and_occts_words_only() {
        use silico_protocol::{DressingKind, EdgeTrouble, Operation, Refusal, Trouble};

        use crate::KernelError;
        let refusals = [
            Refusal::EdgeMissing {
                edge: 13,
                count: 12,
            },
            Refusal::EdgesRefused {
                dressing: DressingKind::Fillet,
                troubles: vec![EdgeTrouble {
                    edge: 3,
                    trouble: Trouble::Smooth,
                }],
            },
            Refusal::ResultInvalid {
                operation: Operation::Extrusion,
                edges: Vec::new(),
            },
        ];
        for refusal in refusals {
            let f = super::failure(KernelError::new(refusal.clone()));
            assert_eq!((f.refusal, f.cause), (refusal, None));
        }
        let occt = Refusal::Occt {
            operation: Operation::Other,
            edges: Vec::new(),
        };
        let f = super::failure(KernelError::caused(
            occt.clone(),
            Some("StdFail_NotDone".to_owned()),
        ));
        assert_eq!(
            (f.refusal, f.cause.as_deref()),
            (occt, Some("StdFail_NotDone"))
        );
    }

    use silico_protocol::{FailureKind, FreecadMeasures, ShapeSummary};

    use super::{check_finite, check_freecad_finite};

    #[test]
    fn a_summary_that_isnt_finite_is_refused_naming_the_value() {
        let good = ShapeSummary {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, 15.0],
            faces: 6,
            edges: 12,
            vertices: 8,
            bounds: Some([[0.0; 3], [10.0, 20.0, 30.0]]),
            valid: true,
        };
        assert!(check_finite(&good).is_ok());
        assert!(
            check_finite(&ShapeSummary {
                bounds: None,
                ..good.clone()
            })
            .is_ok()
        );
        type Spoil = fn(&mut ShapeSummary);
        let cases: [(&str, Spoil); 8] = [
            ("volume isn't finite (NaN)", |s| s.volume = f64::NAN),
            ("volume isn't finite (inf)", |s| s.volume = f64::INFINITY),
            ("area isn't finite (NaN)", |s| s.area = f64::NAN),
            ("center x isn't finite (NaN)", |s| s.center[0] = f64::NAN),
            ("center y isn't finite (-inf)", |s| {
                s.center[1] = f64::NEG_INFINITY
            }),
            ("center z isn't finite (inf)", |s| {
                s.center[2] = f64::INFINITY
            }),
            ("bounding box's lowest y isn't finite (NaN)", |s| {
                s.bounds = Some([[0.0, f64::NAN, 0.0], [10.0, 20.0, 30.0]]);
            }),
            ("bounding box's highest z isn't finite (inf)", |s| {
                s.bounds = Some([[0.0; 3], [10.0, 20.0, f64::INFINITY]]);
            }),
        ];
        for (want, spoil) in cases {
            let mut bad = good.clone();
            spoil(&mut bad);
            let failure = check_finite(&bad).unwrap_err();
            assert_eq!(failure.kind(), FailureKind::KernelFailed);
            let said = failure.refusal.describe(None, &|e| e.to_string());
            assert_eq!(said.message, format!("the shape's {want}"));
        }
        // Every corner of the box, NaN or infinite, each named.
        for (c, corner) in ["lowest", "highest"].iter().enumerate() {
            for (a, axis) in ["x", "y", "z"].iter().enumerate() {
                for (value, text) in [(f64::NAN, "NaN"), (f64::INFINITY, "inf")] {
                    let mut bounds = [[0.0; 3], [10.0, 20.0, 30.0]];
                    bounds[c][a] = value;
                    let bad = ShapeSummary {
                        bounds: Some(bounds),
                        ..good.clone()
                    };
                    let said = check_finite(&bad)
                        .unwrap_err()
                        .refusal
                        .describe(None, &|e| e.to_string());
                    assert_eq!(
                        said.message,
                        format!("the shape's bounding box's {corner} {axis} isn't finite ({text})")
                    );
                }
            }
        }
    }

    #[test]
    fn freecad_measures_that_arent_finite_are_refused() {
        let good = FreecadMeasures {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, 15.0],
            bounds: Some([[0.0; 3], [10.0, 20.0, 30.0]]),
        };
        assert!(check_freecad_finite(&good).is_ok());
        let unbounded = FreecadMeasures {
            bounds: None,
            ..good.clone()
        };
        assert!(check_freecad_finite(&unbounded).is_ok());
        type Spoil = fn(&mut FreecadMeasures);
        let cases: [(&str, Spoil); 6] = [
            ("volume isn't finite (NaN)", |m| m.volume = f64::NAN),
            ("area isn't finite (inf)", |m| m.area = f64::INFINITY),
            ("center y isn't finite (NaN)", |m| m.center[1] = f64::NAN),
            ("center z isn't finite (-inf)", |m| {
                m.center[2] = f64::NEG_INFINITY;
            }),
            ("bounding box's lowest y isn't finite (NaN)", |m| {
                m.bounds = Some([[0.0, f64::NAN, 0.0], [10.0, 20.0, 30.0]]);
            }),
            ("bounding box's highest x isn't finite (inf)", |m| {
                m.bounds = Some([[0.0; 3], [f64::INFINITY, 20.0, 30.0]]);
            }),
        ];
        for (want, spoil) in cases {
            let mut bad = good.clone();
            spoil(&mut bad);
            let failure = check_freecad_finite(&bad).unwrap_err();
            assert_eq!(failure.kind(), FailureKind::KernelFailed);
            let said = failure.refusal.describe(None, &|e| e.to_string());
            assert_eq!(said.message, format!("the shape's {want}"));
        }
    }
}
