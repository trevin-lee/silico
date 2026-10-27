//! The STEP worker (docs/ports/step-export.md, ARCHITECTURE.md's Process model): bodies
//! written as one STEP file by OCCT's XCAF writer, a request at a time. A worker of its own,
//! started for each export, so the kernel worker carries none of STEP's toolkits (nor the
//! windowing frameworks TKService loads on macOS): its startup and memory, the pool's rule for
//! stopping superseded work, and its identity are the kernel's alone. It answers only
//! `ExportStep`; every other operation is another worker's.

// FFI crate (silico-kernel, its STEP bridge): `unsafe` is permitted here, as at the library's
// root (see ARCHITECTURE.md).
#![allow(unsafe_code)]

use std::io::Write;
use std::process::ExitCode;

use silico_kernel::KernelError;
use silico_protocol::{
    ElementKind, ElementRef, ExportStep, Failure, Generation, Op, Refusal, Reply, RequestId,
    StepCount, StepFile, StepUnwritten,
};

#[cxx::bridge(namespace = "silico")]
mod ffi {
    /// What a check-list entry's shape is of a part.
    #[repr(u8)]
    enum StepKind {
        /// The part itself, or of no part.
        Other = 0,
        Face = 1,
        Edge = 2,
        Vertex = 3,
    }

    unsafe extern "C++" {
        include!("silico-kernel/cpp/step.h");

        type StepWriter;
        type StepOut;
        fn step_writer() -> UniquePtr<StepWriter>;
        fn add_part(self: Pin<&mut StepWriter>, brep: &[u8], id: &str, name: &str) -> Result<()>;
        fn write(self: &StepWriter, assembly: &str, file_name: &str) -> Result<UniquePtr<StepOut>>;
        fn bytes(self: &StepOut) -> &[u8];
        fn left_out_counts(self: &StepOut) -> &[u32];
        fn left_out(self: &StepOut) -> &[u32];
        fn unwritten_parts(self: &StepOut) -> &[u32];
        fn unwritten_kinds(self: &StepOut) -> &[StepKind];
        fn unwritten_indices(self: &StepOut) -> &[u32];
        fn unwritten_messages(self: &StepOut) -> Vec<String>;
        fn warnings(self: &StepOut) -> Vec<String>;
        fn faces_expected(self: &StepOut) -> u64;
        fn faces_written(self: &StepOut) -> u64;
        fn bounds_expected(self: &StepOut) -> u64;
        fn bounds_written(self: &StepOut) -> u64;
    }
}

fn main() -> ExitCode {
    silico_kernel::worker::run(|_: RequestId, _: Generation, op: Op, _: &mut dyn Write| handle(op))
}

/// An operation for the STEP worker: `ExportStep` written; anything else refused as another
/// worker's.
fn handle(op: Op) -> Result<Reply, Failure> {
    match op {
        Op::ExportStep(export) => Ok(Reply::StepFile(Box::new(export_step(&export)?))),
        other => Err(Failure::new(Refusal::OtherWorkersOp {
            op: other.name().to_owned(),
        })),
    }
}

/// `export` written as one STEP file, each part a product whose id and name are its
/// strings, already encoded ([`silico_protocol::step_encoded`]), under an assembly named
/// `export.assembly` if there are two or more; the header's file name `export.file_name`, its
/// other fields Silico's constants. Refused if a string isn't in that form (the core encodes
/// them all), a part's B-rep is refused as the kernel refuses one, or OCCT can't transfer or
/// write the parts. Reports each part's faces left out as bounding no material, what else the
/// writer's check list says it failed to transfer, and the faces and their bounds against
/// those written, for the core to refuse; what it warns of, it corrected, and is logged.
fn export_step(export: &ExportStep) -> Result<StepFile, Failure> {
    export.check().map_err(|invalid| {
        Failure::new(Refusal::InvalidParameter {
            parameter: invalid.parameter.to_owned(),
            message: invalid.message,
        })
    })?;
    let mut writer = ffi::step_writer();
    let Some(mut adding) = writer.as_mut() else {
        return Err(internal("the STEP writer couldn't be made"));
    };
    for part in &export.parts {
        let brep = part.shape.brep().as_bytes();
        // Refused as the kernel refuses a B-rep (its checksum, its defects), then read again
        // by the writer from the body the checksum was of.
        silico_kernel::from_brep(brep).map_err(failure)?;
        let body = brep
            .len()
            .checked_sub(4)
            .and_then(|len| brep.get(..len))
            .unwrap_or_default();
        adding
            .as_mut()
            .add_part(body, &part.id, &part.name)
            .map_err(|e| failure(KernelError::from(e)))?;
    }
    let raw = writer
        .write(&export.assembly, &export.file_name)
        .map_err(|e| failure(KernelError::from(e)))?;
    let Some(raw) = raw.as_ref() else {
        return Err(internal("the STEP writer returned nothing"));
    };
    for warning in raw.warnings() {
        tracing::debug!(warning = %warning, "the STEP writer corrected what it wrote");
    }
    Ok(StepFile {
        step: silico_protocol::Blob::inline(raw.bytes().to_vec()),
        left_out: by_part(raw.left_out_counts(), raw.left_out()),
        unwritten: unwritten(
            raw.unwritten_parts(),
            raw.unwritten_kinds(),
            raw.unwritten_indices(),
            raw.unwritten_messages(),
        ),
        faces: StepCount {
            expected: raw.faces_expected(),
            written: raw.faces_written(),
        },
        bounds: StepCount {
            expected: raw.bounds_expected(),
            written: raw.bounds_written(),
        },
    })
}

/// `flat`, a part at a time, `counts[i]` of them for the `i`th.
fn by_part(counts: &[u32], flat: &[u32]) -> Vec<Vec<u32>> {
    let mut rest = flat.iter().copied();
    counts
        .iter()
        .map(|&count| rest.by_ref().take(count as usize).collect())
        .collect()
}

/// The writer's check-list entries, from its columns: the part (none for `u32::MAX`), the
/// element of it (none for the part itself or none), and the messages, one to a line.
fn unwritten(
    parts: &[u32],
    kinds: &[ffi::StepKind],
    indices: &[u32],
    messages: Vec<String>,
) -> Vec<StepUnwritten> {
    parts
        .iter()
        .zip(kinds)
        .zip(indices)
        .zip(messages)
        .map(|(((&part, &kind), &index), messages)| {
            let kind = match kind {
                ffi::StepKind::Face => Some(ElementKind::Face),
                ffi::StepKind::Edge => Some(ElementKind::Edge),
                ffi::StepKind::Vertex => Some(ElementKind::Vertex),
                _ => None,
            };
            StepUnwritten {
                part: (part != u32::MAX).then_some(part),
                element: kind.map(|kind| ElementRef { kind, index }),
                messages: messages.lines().map(str::to_owned).collect(),
            }
        })
        .collect()
}

fn failure(e: KernelError) -> Failure {
    Failure {
        refusal: e.refusal().clone(),
        cause: e.cause().map(str::to_owned),
    }
}

fn internal(message: &str) -> Failure {
    Failure {
        refusal: Refusal::Internal {
            what: message.to_owned(),
        },
        cause: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_parts_faces_left_out_are_its_own() {
        assert_eq!(
            by_part(&[0, 2, 1], &[5, 6, 9]),
            [vec![], vec![5, 6], vec![9]]
        );
        assert_eq!(by_part(&[], &[]), Vec::<Vec<u32>>::new());
    }

    #[test]
    fn a_check_list_entry_is_its_part_its_element_and_its_messages() {
        // Each kind as the bridge numbers it (StepKind, shared with step.h), a part of none
        // (u32::MAX), and messages one to a line.
        let got = unwritten(
            &[0, 1, 2, u32::MAX],
            &[
                ffi::StepKind::Face,
                ffi::StepKind::Edge,
                ffi::StepKind::Vertex,
                ffi::StepKind::Other,
            ],
            &[3, 7, 1, 0],
            vec![
                "a\nb".to_owned(),
                "c".to_owned(),
                String::new(),
                "d".to_owned(),
            ],
        );
        let element = |kind, index| Some(ElementRef { kind, index });
        assert_eq!(
            got,
            [
                StepUnwritten {
                    part: Some(0),
                    element: element(ElementKind::Face, 3),
                    messages: vec!["a".to_owned(), "b".to_owned()],
                },
                StepUnwritten {
                    part: Some(1),
                    element: element(ElementKind::Edge, 7),
                    messages: vec!["c".to_owned()],
                },
                StepUnwritten {
                    part: Some(2),
                    element: element(ElementKind::Vertex, 1),
                    messages: vec![],
                },
                StepUnwritten {
                    part: None,
                    element: None,
                    messages: vec!["d".to_owned()],
                },
            ]
        );
    }

    #[test]
    fn a_string_not_encoded_is_refused_naming_its_field_before_any_part_is_read() {
        // The core never sends one; the worker refuses it before OCCT reads it as a
        // directive, and before it reads the (here empty) B-rep.
        let failure = handle(Op::ExportStep(Box::new(ExportStep {
            parts: vec![silico_protocol::StepPart {
                shape: silico_protocol::ShapeData::unchecked(silico_protocol::Blob::inline(vec![])),
                id: "k7f2qx3m".to_owned(),
                name: "a\\N\\b".to_owned(),
            }],
            assembly: "bracket".to_owned(),
            file_name: "bracket.silico".to_owned(),
        })))
        .unwrap_err();
        assert!(
            matches!(
                &failure.refusal,
                Refusal::InvalidParameter { parameter, .. } if parameter == "name"
            ),
            "{failure:?}"
        );
    }

    #[test]
    fn an_operation_but_export_step_is_another_workers() {
        let failure = handle(Op::Summarize {
            shape: silico_protocol::ShapeData::unchecked(silico_protocol::Blob::inline(vec![])),
        })
        .unwrap_err();
        assert!(
            matches!(&failure.refusal, Refusal::OtherWorkersOp { op } if op == "Summarize"),
            "{failure:?}"
        );
    }
}
