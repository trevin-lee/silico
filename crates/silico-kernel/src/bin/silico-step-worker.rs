//! The STEP worker (docs/ports/step-export.md, ARCHITECTURE.md's Process model): bodies
//! written as one STEP file by OCCT's XCAF writer, a request at a time. A worker of its own,
//! started for each export, so the kernel worker carries none of STEP's toolkits (nor the
//! windowing frameworks TKService loads on macOS): its startup and memory, the pool's rule for
//! stopping superseded work, and its identity are the kernel's alone. It answers only
//! `ExportStep`; every other operation is another worker's.

// FFI crate (silico-kernel, its STEP bridge): `unsafe` is permitted here, as at the library's
// root (see ARCHITECTURE.md).
#![allow(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
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
/// other fields Silico's constants. OCCT is given a token for each string but the ids, which
/// are spliced into the text it writes ([`Strings`]). Refused if a string isn't in that form
/// (the core encodes them all), a part's B-rep is refused as the kernel refuses one, or OCCT
/// can't transfer or write the parts, or its text doesn't hold the tokens it was given.
/// Reports each part's faces left out as bounding no material, what else the writer's check
/// list says it failed to transfer, and the faces and their bounds against those written,
/// for the core to refuse; what it warns of, it corrected, and is logged.
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
    let mut strings = Strings::default();
    for (index, part) in export.parts.iter().enumerate() {
        let brep = part.shape.brep().as_bytes();
        // Refused as the kernel refuses a B-rep (its checksum, its defects), then read again
        // by the writer from the body the checksum was of.
        silico_kernel::from_brep(brep).map_err(failure)?;
        let body = brep
            .len()
            .checked_sub(4)
            .and_then(|len| brep.get(..len))
            .unwrap_or_default();
        let name = strings.token("NAME", index, &part.name);
        adding
            .as_mut()
            .add_part(body, &part.id, &name)
            .map_err(|e| failure(KernelError::from(e)))?;
    }
    // The writer names an assembly only for two parts or more.
    let assembly = if export.parts.len() > 1 {
        strings.token("ASSEMBLY", 0, &export.assembly)
    } else {
        String::new()
    };
    let file_name = strings.token("FILE", 0, &export.file_name);
    let raw = writer
        .write(&assembly, &file_name)
        .map_err(|e| failure(KernelError::from(e)))?;
    let Some(raw) = raw.as_ref() else {
        return Err(internal("the STEP writer returned nothing"));
    };
    for warning in raw.warnings() {
        tracing::debug!(warning = %warning, "the STEP writer corrected what it wrote");
    }
    let step = strings
        .splice(raw.bytes())
        .map_err(|fault| internal(&fault.what()))?;
    Ok(StepFile {
        step: silico_protocol::Blob::inline(step),
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

/// The strings a STEP file holds (docs/ports/step-export.md, *Strings*), by the token OCCT is
/// given in place of each: `SILICO-<what>-<index>`, which no shape of OCCT's names anything,
/// short enough that OCCT never wraps it. OCCT breaks a string past 72 columns, inside an
/// encoded run too, and its reader decodes before it joins the lines, so OCCT writes none of
/// Silico's strings itself: each is spliced in where its token is, once the file is written.
#[derive(Debug, Default)]
struct Strings<'a> {
    by_token: BTreeMap<String, &'a str>,
}

/// What keeps a STEP file's strings from being spliced in: OCCT's text is not what it was
/// given, a fault of the worker's.
#[derive(Debug, PartialEq, Eq)]
enum SpliceFault {
    /// A `'SILICO-` with no `'` after it.
    Unclosed,
    /// A token no string was given for (lossily, if it isn't UTF-8).
    Unknown(String),
    /// A token given that the text doesn't hold.
    Missing(String),
}

impl SpliceFault {
    fn what(&self) -> String {
        match self {
            Self::Unclosed => "OCCT's STEP writer left a placeholder unclosed".to_owned(),
            Self::Unknown(token) => {
                format!("OCCT's STEP writer wrote a placeholder Silico didn't give: {token}")
            }
            Self::Missing(token) => format!("OCCT's STEP writer wrote no {token}"),
        }
    }
}

impl<'a> Strings<'a> {
    /// The token for `value`, the `index`th string of its kind `what`, which [`Strings::splice`]
    /// replaces with it.
    fn token(&mut self, what: &str, index: usize, value: &'a str) -> String {
        let token = format!("SILICO-{what}-{index}");
        self.by_token.insert(token.clone(), value);
        token
    }

    /// `text` with each `'TOKEN'` (a `'SILICO-` to the next `'`) made `'VALUE'`, in one pass,
    /// so it's as long as the file however many there are; values as given (in
    /// [`silico_protocol::step_encoded`]'s form, which has no `'`, as `ExportStep::check`
    /// makes sure), never searched again. Refused if a token is unclosed or unknown, or one
    /// given isn't found.
    fn splice(&self, text: &[u8]) -> Result<Vec<u8>, SpliceFault> {
        const OPEN: &[u8] = b"'SILICO-";
        let mut out = Vec::with_capacity(text.len());
        let mut found = BTreeSet::new();
        let mut rest = text;
        while let Some(next) = rest.windows(OPEN.len()).position(|w| w == OPEN) {
            let (before, quoted) = rest.split_at(next);
            let after_open = quoted.get(1..).unwrap_or_default();
            let Some(close) = after_open.iter().position(|&b| b == b'\'') else {
                return Err(SpliceFault::Unclosed);
            };
            let (token, closed) = after_open.split_at(close);
            let Some((key, value)) = std::str::from_utf8(token)
                .ok()
                .and_then(|token| self.by_token.get_key_value(token))
            else {
                return Err(SpliceFault::Unknown(
                    String::from_utf8_lossy(token).into_owned(),
                ));
            };
            out.extend_from_slice(before);
            out.push(b'\'');
            out.extend_from_slice(value.as_bytes());
            out.push(b'\'');
            found.insert(key.as_str());
            rest = closed.get(1..).unwrap_or_default();
        }
        out.extend_from_slice(rest);
        match self
            .by_token
            .keys()
            .find(|token| !found.contains(token.as_str()))
        {
            Some(token) => Err(SpliceFault::Missing(token.clone())),
            None => Ok(out),
        }
    }
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

    /// The strings a two-part export gives, by their tokens, as `export_step` makes them.
    fn two_parts<'a>(names: [&'a str; 2], assembly: &'a str, file: &'a str) -> Strings<'a> {
        let mut strings = Strings::default();
        assert_eq!(strings.token("NAME", 0, names[0]), "SILICO-NAME-0");
        assert_eq!(strings.token("NAME", 1, names[1]), "SILICO-NAME-1");
        assert_eq!(strings.token("ASSEMBLY", 0, assembly), "SILICO-ASSEMBLY-0");
        assert_eq!(strings.token("FILE", 0, file), "SILICO-FILE-0");
        strings
    }

    #[test]
    fn each_token_is_replaced_by_its_string_wherever_it_is() {
        let strings = two_parts(
            ["Bracket", "Kl\\X2\\00F6\\X0\\tzchen"],
            "bracket",
            "bracket.silico",
        );
        let text = b"FILE_NAME('SILICO-FILE-0','1970');\n\
            #1=PRODUCT('k7','SILICO-NAME-0','SILICO-NAME-0',(#9));\n\
            #2=PRODUCT('q3','SILICO-NAME-1','SILICO-NAME-1',(#9));\n\
            #3=PRODUCT('','SILICO-ASSEMBLY-0','SILICO-ASSEMBLY-0',(#9));\n";
        let want = b"FILE_NAME('bracket.silico','1970');\n\
            #1=PRODUCT('k7','Bracket','Bracket',(#9));\n\
            #2=PRODUCT('q3','Kl\\X2\\00F6\\X0\\tzchen','Kl\\X2\\00F6\\X0\\tzchen',(#9));\n\
            #3=PRODUCT('','bracket','bracket',(#9));\n";
        assert_eq!(
            String::from_utf8_lossy(&strings.splice(text).unwrap()),
            String::from_utf8_lossy(want)
        );
    }

    #[test]
    fn an_empty_string_and_a_token_at_either_end_are_spliced() {
        let mut strings = Strings::default();
        strings.token("NAME", 0, "");
        strings.token("FILE", 0, "f");
        assert_eq!(
            strings.splice(b"'SILICO-NAME-0' 'SILICO-FILE-0'").unwrap(),
            b"'' 'f'"
        );
    }

    #[test]
    fn text_without_tokens_is_returned_as_it_is_bytes_that_arent_utf8_too() {
        let text = b"ISO-10303-21;\n#1=PRODUCT('a','\xff\xfe',());\nSILICO-NAME-0 'SILICO";
        assert_eq!(Strings::default().splice(text).unwrap(), text);
        assert_eq!(Strings::default().splice(b"").unwrap(), b"");
    }

    #[test]
    fn a_token_given_but_never_written_is_refused_by_its_name() {
        let mut strings = Strings::default();
        strings.token("NAME", 0, "Bracket");
        strings.token("FILE", 0, "bracket.silico");
        // The name's token is in the text, but not quoted: it isn't one.
        let fault = strings
            .splice(b"FILE_NAME('SILICO-FILE-0'); #1=PRODUCT(SILICO-NAME-0);")
            .unwrap_err();
        assert_eq!(fault, SpliceFault::Missing("SILICO-NAME-0".to_owned()));
        assert_eq!(fault.what(), "OCCT's STEP writer wrote no SILICO-NAME-0");
        // Nothing at all written.
        assert_eq!(
            strings.splice(b"").unwrap_err(),
            SpliceFault::Missing("SILICO-FILE-0".to_owned())
        );
    }

    #[test]
    fn a_token_not_given_is_refused_by_its_name_even_one_a_given_one_begins() {
        let mut strings = Strings::default();
        strings.token("NAME", 1, "Bracket");
        let fault = strings
            .splice(b"'SILICO-NAME-1' 'SILICO-NAME-10'")
            .unwrap_err();
        assert_eq!(fault, SpliceFault::Unknown("SILICO-NAME-10".to_owned()));
        assert_eq!(
            fault.what(),
            "OCCT's STEP writer wrote a placeholder Silico didn't give: SILICO-NAME-10"
        );
        assert_eq!(
            strings.splice(b"'SILICO-' 'SILICO-NAME-1'").unwrap_err(),
            SpliceFault::Unknown("SILICO-".to_owned())
        );
        // Bytes that aren't UTF-8, lossily.
        assert_eq!(
            strings.splice(b"'SILICO-\xff'").unwrap_err(),
            SpliceFault::Unknown("SILICO-\u{fffd}".to_owned())
        );
    }

    #[test]
    fn a_token_never_closed_is_refused() {
        let mut strings = Strings::default();
        strings.token("FILE", 0, "bracket.silico");
        for text in [
            &b"FILE_NAME('SILICO-FILE-0);"[..],
            b"'SILICO-",
            b"'SILICO-FILE-0' 'SILICO-FILE-0",
        ] {
            let fault = strings.splice(text).unwrap_err();
            assert_eq!(
                fault,
                SpliceFault::Unclosed,
                "{}",
                String::from_utf8_lossy(text)
            );
            assert_eq!(
                fault.what(),
                "OCCT's STEP writer left a placeholder unclosed"
            );
        }
    }

    #[test]
    fn a_string_that_looks_like_a_token_is_written_as_it_is_and_never_read_again() {
        // A name that is another's token, and one that is a quoted token's start: each is
        // written, never itself replaced, and the text's own tokens are still each found.
        let strings = two_parts(
            ["SILICO-NAME-1", "SILICO-"],
            "SILICO-FILE-0",
            "SILICO-ASSEMBLY-0",
        );
        let text = b"'SILICO-NAME-0','SILICO-NAME-1','SILICO-ASSEMBLY-0','SILICO-FILE-0'";
        assert_eq!(
            String::from_utf8_lossy(&strings.splice(text).unwrap()),
            "'SILICO-NAME-1','SILICO-','SILICO-FILE-0','SILICO-ASSEMBLY-0'"
        );
        // Text that looks like a token's start but isn't one: no quote, or a short one.
        let mut file = Strings::default();
        file.token("FILE", 0, "f");
        assert_eq!(
            String::from_utf8_lossy(
                &file
                    .splice(b"SILICO-FILE-0 'SILICO 'SILICO_FILE-0' ''SILICO-FILE-0'")
                    .unwrap()
            ),
            "SILICO-FILE-0 'SILICO 'SILICO_FILE-0' ''f'"
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
