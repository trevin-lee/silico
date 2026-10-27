//! break-it: M4.8's STEP export in the core (`command::export`'s `stepped` and
//! `unwritten`, `KernelRequest::export_step`'s reply checks) on the fake kernel: hostile
//! document paths for the header and the assembly, a worker's reply of any shape (a
//! property test: every reply either written and answered as it says, or refused with a
//! structured error and nothing written), and what the worker's file-wide face count is
//! said against. Expected values are by hand from the request and the reply.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::example::EXAMPLE;

use proptest::prelude::*;
use silico_core::command::export::{Export, ExportFormat, ExportOn, Exported, export};
use silico_core::command::{self, Problems};
use silico_core::document::FeatureId;
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::recompute::{Model, Recompute};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Blob, ElementKind, ElementRef, Reply, StepFile, StepUnwritten};

/// EXAMPLE's model on its own fake kernel, whose STEP replies are `reply`'s, if it holds
/// one, else the fake's own.
struct Bench {
    kernel: Arc<FakeKernel>,
    model: Arc<Model>,
    reply: Arc<Mutex<Option<StepFile>>>,
}

impl Bench {
    fn new() -> Bench {
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        let reply: Arc<Mutex<Option<StepFile>>> = Arc::new(Mutex::new(None));
        let answers = Arc::clone(&reply);
        kernel.answer_instead(move |op| match op {
            Op::ExportStep(_) => answers
                .lock()
                .unwrap()
                .clone()
                .map(|file| Reply::StepFile(Box::new(file))),
            _ => None,
        });
        let engine = Recompute::new(Arc::clone(&kernel));
        let model = command::rebuild(&engine, command::gate(EXAMPLE.as_bytes()).unwrap())
            .unwrap()
            .unwrap();
        Bench {
            kernel,
            model,
            reply,
        }
    }

    fn export(&self, document: &str, features: &[&str], path: &Path) -> Result<Exported, Problems> {
        export(
            &self.model,
            &Export {
                format: ExportFormat::Step,
                path: path.to_owned(),
                replace: true,
                features: Some(features.iter().map(|&f| id(f)).collect()),
            },
            &ExportOn {
                kernel: &self.kernel,
                step: &*self.kernel,
                document,
                workspace: None,
                held: &[],
                settle: &|| Ok(()),
            },
        )
    }

    /// How many faces `feature`'s element map names, from Face1 on.
    fn faces(&self, feature: &str) -> u32 {
        let map = self.model.element_map(&id(feature)).unwrap();
        (1..)
            .take_while(|&n| map.name(ElementRef::face(n)).is_some())
            .count() as u32
    }
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// A directory of its own for one test, removed after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "silico-adversarial-step-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(std::fs::canonicalize(&dir).unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_hostile_document_path_is_encoded_into_the_header_and_the_assembly() {
    // The header's FILE_NAME is the path's last component, and the assembly the stem, each
    // encoded: an umlaut and an apostrophe, a backslash that looks like a directive (a file
    // name on Unix may hold one), a dotfile, a path with no file name, an empty path.
    let bench = Bench::new();
    let dir = Scratch::new("paths");
    let out = dir.0.join("out.step");
    for (document, file_name, assembly) in [
        (
            "/home/u/Kl\u{F6}tzchen 'v2'.silico",
            "Kl\\X2\\00F6\\X0\\tzchen \\X2\\0027\\X0\\v2\\X2\\0027\\X0\\.silico",
            "Kl\\X2\\00F6\\X0\\tzchen \\X2\\0027\\X0\\v2\\X2\\0027\\X0\\",
        ),
        (
            "dir/C:\\X2\\temp.silico",
            "C:\\X2\\005C\\X0\\X2\\X2\\005C\\X0\\temp.silico",
            "C:\\X2\\005C\\X0\\X2\\X2\\005C\\X0\\temp",
        ),
        ("dir/.silico", ".silico", ".silico"),
        ("dir/..", "", ""),
        ("", "", ""),
    ] {
        bench
            .export(document, &["m3tq8v0z", "k7f2qx3m"], &out)
            .unwrap_or_else(|p| panic!("{document:?}: {:?}", p.errors()));
        let file = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = file.lines().collect();
        assert_eq!(
            lines[1],
            format!("FILE_NAME('{file_name}')"),
            "{document:?}"
        );
        assert_eq!(lines[2], format!("ASSEMBLY('{assembly}')"), "{document:?}");
    }
}

#[test]
fn a_one_part_files_face_shortfall_is_refused_naming_its_feature() {
    // The worker's count of faces written against faces that bound material is the file's,
    // of no part. With one part it can only be that part's: the design's refusal names the
    // feature ("invalid_result naming the feature and the elements"), as `stepped` already
    // names it for a kernel error of a one-part export.
    // BREAKS: the error names no feature (and says "couldn't write all of the file"): an
    // agent isn't told which body to rebuild or export as 3MF.
    let bench = Bench::new();
    let dir = Scratch::new("shortfall");
    *bench.reply.lock().unwrap() = Some(StepFile {
        step: Blob::inline(b"STEP".to_vec()),
        left_out: vec![vec![]],
        unwritten: vec![],
        faces: silico_protocol::StepCount {
            expected: 6,
            written: 5,
        },
        bounds: silico_protocol::StepCount {
            expected: 6,
            written: 6,
        },
    });
    let out = dir.0.join("out.step");
    let problems = bench
        .export("part.silico", &["k7f2qx3m"], &out)
        .unwrap_err();
    assert!(!out.exists());
    let e = &problems.errors()[0];
    assert_eq!(e.code(), ErrorCode::InvalidResult, "{e:?}");
    assert_eq!(e.feature(), Some("k7f2qx3m"), "{e:?}");
}

/// A reply of the worker's: for `parts` parts, faces left out by part, and unwritten
/// entries, each index drawn around the edges (0, 1, the last face, one past it, the part
/// count, u32::MAX).
fn replies() -> impl Strategy<Value = (usize, StepFile)> {
    let index = prop_oneof![
        Just(0u32),
        Just(1u32),
        Just(6u32),
        Just(7u32),
        Just(9u32),
        Just(10u32),
        Just(u32::MAX),
        1u32..12,
    ];
    let part = prop_oneof![
        Just(None),
        Just(Some(0u32)),
        Just(Some(1u32)),
        Just(Some(2u32)),
        Just(Some(u32::MAX)),
    ];
    let kind = prop_oneof![
        Just(ElementKind::Face),
        Just(ElementKind::Edge),
        Just(ElementKind::Vertex)
    ];
    let element = proptest::option::of(
        (kind, index.clone()).prop_map(|(kind, index)| ElementRef { kind, index }),
    );
    let unwritten = proptest::collection::vec(
        (
            part,
            element,
            proptest::collection::vec(prop_oneof![Just(String::new()), "[ -~]{0,8}"], 0..3),
        )
            .prop_map(|(part, element, messages)| StepUnwritten {
                part,
                element,
                messages,
            }),
        0..4,
    );
    (
        1usize..=2,
        proptest::collection::vec(proptest::collection::vec(index, 0..4), 0..4),
        prop_oneof![3 => Just(Vec::new()), 1 => unwritten],
    )
        .prop_map(|(parts, left_out, unwritten)| {
            (
                parts,
                StepFile {
                    step: Blob::inline(b"ISO-10303-21;".to_vec()),
                    left_out,
                    unwritten,
                    // Whole: a shortfall's refusal is export_command.rs's to test.
                    faces: silico_protocol::StepCount {
                        expected: 6,
                        written: 6,
                    },
                    bounds: silico_protocol::StepCount {
                        expected: 6,
                        written: 6,
                    },
                },
            )
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn every_worker_reply_is_written_as_it_says_or_refused_with_nothing_written(
        (parts, reply) in replies()
    ) {
        // The block (k7f2qx3m) then the cut (m3tq8v0z), or the block alone.
        thread_local!(static BENCH: Bench = Bench::new());
        BENCH.with(|bench| {
            let features: &[&str] = if parts == 2 { &["k7f2qx3m", "m3tq8v0z"] } else { &["k7f2qx3m"] };
            let faces: Vec<u32> = features.iter().map(|f| bench.faces(f)).collect();
            *bench.reply.lock().unwrap() = Some(reply.clone());
            let dir = Scratch::new("property");
            let out = dir.0.join("out.step");
            let result = bench.export("part.silico", features, &out);
            // What the reply should give, by hand.
            let malformed = reply.left_out.len() != parts
                || reply.left_out.iter().flatten().any(|&f| f == 0)
                || reply.unwritten.iter().any(|u| {
                    u.part.is_some_and(|p| p as usize >= parts)
                        || u.element.is_some_and(|e| e.index == 0)
                });
            match result {
                Ok(done) => {
                    prop_assert!(!malformed, "{reply:?}");
                    prop_assert!(reply.unwritten.is_empty(), "{reply:?}");
                    prop_assert_eq!(std::fs::read(&out).unwrap(), reply.step.as_bytes());
                    prop_assert_eq!(done.objects.len(), parts);
                    for (n, object) in done.objects.iter().enumerate() {
                        prop_assert!(reply.left_out[n].iter().all(|&f| f <= faces[n]));
                        prop_assert_eq!(object.left_out.len(), reply.left_out[n].len());
                        prop_assert!(object.mesh.is_none());
                    }
                }
                Err(problems) => {
                    prop_assert!(!out.exists(), "written though refused: {reply:?}");
                    prop_assert!(!problems.errors().is_empty());
                    let codes: Vec<ErrorCode> = problems.errors().iter().map(Error::code).collect();
                    if malformed {
                        prop_assert_eq!(&codes, &[ErrorCode::ProtocolError]);
                    } else if !reply.unwritten.is_empty() {
                        // One error a part said of (and one for what's of none), each
                        // invalid_result, each a part's naming its feature.
                        let mut said: Vec<Option<u32>> = reply.unwritten.iter().map(|u| u.part).collect();
                        said.sort_unstable();
                        said.dedup();
                        prop_assert_eq!(problems.errors().len(), said.len(), "{:?}", problems.errors());
                        for (e, part) in problems.errors().iter().zip(&said) {
                            prop_assert_eq!(e.code(), ErrorCode::InvalidResult);
                            if let Some(p) = part {
                                prop_assert_eq!(e.feature(), Some(features[*p as usize]));
                            }
                            prop_assert!(e.suggestion().is_some());
                            prop_assert!(e.cause().is_some());
                        }
                    } else {
                        // A face left out that the part hasn't: the core's own check.
                        prop_assert!(
                            reply.left_out.iter().zip(&faces).any(|(l, &n)| l.iter().any(|&f| f > n)),
                            "{:?} for {reply:?}", problems.errors()
                        );
                        prop_assert_eq!(&codes, &[ErrorCode::Internal]);
                    }
                }
            }
            Ok(())
        })?;
    }
}
