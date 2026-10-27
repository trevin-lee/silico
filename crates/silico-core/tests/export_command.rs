//! M4.8's `export` command (docs/design/m4.1-command-api.md, `export`) on the fake kernel:
//! each solid's export mesh answered with a tetrahedron built by hand (export_report.rs),
//! as the worker reports a B-rep's meshed faces, so the command's own steps are what's
//! tested (its checks in the design's order, its objects and their order, the file written
//! once its read settles, and nothing written for a request refused). The real kernel's
//! meshes, and the archive a slicer reads, are silico-kernel's tests/export.rs, and the CLI's
//! export of them silico-cli's.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::example::EXAMPLE;
use super::export_report::{tetra, unasked};

use silico_core::command::export::{Export, ExportFormat, ExportOn, Exported, export};
use silico_core::command::{self, Problems};
use silico_core::document::{FeatureId, Length, Measure};
use silico_core::export::{Object, printable, three_mf};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::recompute::{Model, Recompute};
use silico_core::{Error, ErrorCode};
use silico_protocol::Reply;

/// A model rebuilt on its own fake kernel, whose export meshes are the tetrahedron.
struct Bench {
    kernel: Arc<FakeKernel>,
    model: Arc<Model>,
}

impl Bench {
    fn of(text: &str) -> Bench {
        Bench::with(text, |_| {})
    }

    /// [`Bench::of`], the fake kernel set up by `setup` before the rebuild.
    fn with(text: &str, setup: impl FnOnce(&FakeKernel)) -> Bench {
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        setup(&kernel);
        kernel.answer_instead(|op| match op {
            Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(tetra()))),
            _ => None,
        });
        let engine = Recompute::new(Arc::clone(&kernel));
        let model = command::rebuild(&engine, command::gate(text.as_bytes()).unwrap())
            .unwrap()
            .unwrap();
        Bench { kernel, model }
    }

    fn export(&self, request: &Export) -> Result<Exported, Problems> {
        self.export_settling(request, &|| Ok(()))
    }

    fn export_settling(
        &self,
        request: &Export,
        settle: &dyn Fn() -> Result<(), Error>,
    ) -> Result<Exported, Problems> {
        export(
            &self.model,
            request,
            &ExportOn {
                kernel: &self.kernel,
                step: &*self.kernel,
                document: "part.silico",
                workspace: None,
                held: &[],
                turns: &silico_core::command::export::MeshTurns::new(),
                stopped: &|| false,
                settle,
            },
        )
    }

    /// The export meshes submitted to the kernel so far, whether or not they started.
    fn asked_meshes(&self) -> usize {
        self.kernel
            .asked()
            .iter()
            .filter(|c| matches!(c.op, Op::ExportMesh { .. }))
            .count()
    }

    /// The export meshes asked of the kernel so far.
    fn meshed(&self) -> usize {
        self.kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::ExportMesh { .. }))
            .count()
    }
}

/// A directory of its own for one test, removed after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "silico-export-command-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(std::fs::canonicalize(&dir).unwrap())
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request(path: &Path) -> Export {
    Export {
        format: ExportFormat::ThreeMf {
            tolerance: Length::rounded(0.1).unwrap(),
        },
        path: path.to_owned(),
        replace: false,
        features: None,
    }
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// The archive of the tetrahedron as `names` (each a feature's name and ID), as the export
/// core writes it: what the command must have written for them.
fn archive_of(names: &[(&str, &str)]) -> Vec<u8> {
    let mesh = printable(&tetra(), unasked, None).unwrap();
    let objects: Vec<Object<'_>> = names
        .iter()
        .map(|&(name, part_number)| Object {
            name,
            part_number,
            mesh: &mesh,
        })
        .collect();
    three_mf(&objects).unwrap()
}

fn refused(result: Result<Exported, Problems>) -> Problems {
    match result {
        Ok(done) => panic!("written: {done:?}"),
        Err(problems) => problems,
    }
}

#[test]
fn every_body_is_written_one_object_each_and_answered() {
    // The example's one body is the cut, "Hole" (m3tq8v0z): the block and the pin it
    // consumed aren't bodies. Its object is the tetrahedron: 4 triangles, 4 vertices, a
    // sixth of a cubic millimetre (det(P1, P2, P3) / 6), written to 6 decimals.
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("bodies");
    let path = dir.0.join("out.3mf");
    let done = bench.export(&request(&path)).unwrap();
    let archive = archive_of(&[("Hole", "m3tq8v0z")]);
    assert_eq!(std::fs::read(&path).unwrap(), archive);
    assert_eq!(
        done,
        Exported {
            path: path.clone(),
            bytes: archive.len() as u64,
            objects: vec![silico_core::command::export::ExportedObject {
                feature: id("m3tq8v0z"),
                name: "Hole".to_owned(),
                mesh: Some(silico_core::command::export::MeshWritten {
                    triangles: 4,
                    vertices: 4,
                    volume: Measure::parse("0.166667").unwrap(),
                }),
                left_out: vec![],
            }],
        }
    );
    assert_eq!(bench.meshed(), 1);
    // Nothing left beside it.
    assert_eq!(dir.names(), ["out.3mf"]);
}

#[test]
fn features_listed_are_written_in_the_lists_order_built_before_they_were_consumed() {
    // The cut, then the block it consumed (as it was built, before the cut), against the
    // timeline's order: the archive's objects, and the answer's, in the order asked. (The
    // pin's fake solid has three faces, too few to stand for the tetrahedron.)
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("listed");
    let path = dir.0.join("parts.3mf");
    let mut asked = request(&path);
    asked.features = Some(vec![id("m3tq8v0z"), id("k7f2qx3m")]);
    let done = bench.export(&asked).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        archive_of(&[("Hole", "m3tq8v0z"), ("Block", "k7f2qx3m")])
    );
    let objects: Vec<(&str, &str)> = done
        .objects
        .iter()
        .map(|o| (o.feature.as_str(), o.name.as_str()))
        .collect();
    assert_eq!(objects, [("m3tq8v0z", "Hole"), ("k7f2qx3m", "Block")]);
}

#[test]
fn a_tolerance_out_of_range_is_refused_in_the_designs_words_asking_the_kernel_nothing() {
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("tolerance");
    let mut asked = request(&dir.0.join("out.3mf"));
    asked.format = ExportFormat::ThreeMf {
        tolerance: Length::rounded(0.0).unwrap(),
    };
    let problems = refused(bench.export(&asked));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.message(), e.parameter(), e.suggestion()),
        (
            ErrorCode::InvalidInput,
            "tolerance must be a length from 0.00001 mm to 100000000 mm, got 0",
            Some("tolerance"),
            Some("give a tolerance in range; 0.1 mm is FreeCAD's default")
        )
    );
    assert_eq!(bench.meshed(), 0);
    assert!(dir.names().is_empty());
}

#[test]
fn every_reason_a_request_is_refused_is_said_at_once_in_the_designs_order() {
    // A tolerance out of range, a path ending otherwise than .3mf, and the features: one
    // the document hasn't, and one listed twice. Nothing meshed, nothing written.
    let text = EXAMPLE;
    let bench = Bench::of(text);
    let dir = Scratch::new("every");
    let mut asked = request(&dir.0.join("out.stl"));
    asked.format = ExportFormat::ThreeMf {
        tolerance: Length::rounded(1e9).unwrap(),
    };
    asked.features = Some(vec![id("k7f2qx3m"), id("zzzzzzzz"), id("k7f2qx3m")]);
    let problems = refused(bench.export(&asked));
    let said: Vec<(Option<&str>, Option<&str>)> = problems
        .errors()
        .iter()
        .map(|e| (e.parameter(), e.feature()))
        .collect();
    assert_eq!(
        said,
        [
            (Some("tolerance"), None),
            (Some("path"), None),
            (Some("features.1"), None),
            (Some("features.2"), Some("k7f2qx3m")),
        ]
    );
    assert_eq!(problems.total(), 4);
    assert_eq!(bench.meshed(), 0);
    assert!(dir.names().is_empty());
}

#[test]
fn no_features_listed_is_refused_naming_features() {
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("none-listed");
    let mut asked = request(&dir.0.join("out.3mf"));
    asked.features = Some(vec![]);
    let problems = refused(bench.export(&asked));
    assert_eq!(problems.errors()[0].parameter(), Some("features"));
    assert_eq!(bench.meshed(), 0);
}

#[test]
fn a_file_already_there_is_kept_unless_replace_is_given() {
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("exists");
    let path = dir.0.join("out.3mf");
    std::fs::write(&path, b"someone's").unwrap();
    let problems = refused(bench.export(&request(&path)));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("path"))
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"someone's");
    assert_eq!(bench.meshed(), 0);
    let mut replacing = request(&path);
    replacing.replace = true;
    bench.export(&replacing).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        archive_of(&[("Hole", "m3tq8v0z")])
    );
}

#[test]
fn a_document_as_the_path_is_refused_and_kept() {
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("document");
    let path = dir.0.join("bracket.silico");
    std::fs::write(&path, EXAMPLE).unwrap();
    let mut asked = request(&path);
    asked.replace = true;
    let problems = refused(bench.export(&asked));
    assert_eq!(problems.errors()[0].parameter(), Some("path"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
}

/// A document of one sketch, a 10 mm square on XY.
const SKETCH: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Square", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 10, "y": 10, "construction": false}, {"id": "p0000004", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}, {"id": "n0000003", "from": "p0000003", "to": "p0000004", "construction": false}, {"id": "n0000004", "from": "p0000004", "to": "p0000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}]}"#;

#[test]
fn a_feature_without_a_solid_is_refused_by_its_place_in_the_list() {
    // A sketch makes no solid: refused naming `features.0` and the sketch. And with no
    // features listed, a model of a sketch alone has no bodies to write.
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    let engine =
        Recompute::new(Arc::clone(&kernel)).with_sketches(silico_core::fake::FakeSketches::new());
    let model = command::rebuild(&engine, command::gate(SKETCH.as_bytes()).unwrap())
        .unwrap()
        .unwrap();
    let bench = Bench { kernel, model };
    let dir = Scratch::new("sketch");
    let mut asked = request(&dir.0.join("out.3mf"));
    asked.features = Some(vec![id("s0000001")]);
    let problems = refused(bench.export(&asked));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (
            ErrorCode::InvalidInput,
            Some("features.0"),
            Some("s0000001")
        ),
        "{e:?}"
    );
    asked.features = None;
    let problems = refused(bench.export(&asked));
    assert!(
        problems.errors()[0].message().contains("no bodies"),
        "{:?}",
        problems.errors()
    );
    assert_eq!(bench.meshed(), 0);
    assert!(dir.names().is_empty());
}

#[test]
fn a_name_a_3mf_file_cant_hold_is_refused_naming_the_feature_and_name() {
    // The cut renamed with a control character (U+0001), which XML 1.0 can't hold.
    let text = EXAMPLE.replace("\"name\": \"Hole\"", "\"name\": \"Ho\\u0001le\"");
    let bench = Bench::of(&text);
    let dir = Scratch::new("name");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.feature(), e.parameter()),
        (ErrorCode::InvalidInput, Some("m3tq8v0z"), Some("name")),
        "{e:?}"
    );
    assert_eq!(bench.meshed(), 0);
    assert!(dir.names().is_empty());
}

#[test]
fn an_export_whose_read_wont_settle_writes_no_file() {
    // Meshed and archived, then the read cancelled: its cancel is the answer, nothing
    // written (decision 34: a cancelled command leaves no trace).
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("settle");
    let path = dir.0.join("out.3mf");
    let cancelled = || Err(Error::new(ErrorCode::Cancelled, "the export was cancelled"));
    let problems = refused(bench.export_settling(&request(&path), &cancelled));
    assert_eq!(problems.errors()[0].code(), ErrorCode::Cancelled);
    assert_eq!(bench.meshed(), 1);
    assert!(dir.names().is_empty());
}

#[test]
fn a_mesh_the_kernel_refuses_names_its_feature_and_writes_nothing() {
    let bench = Bench::of(EXAMPLE);
    bench.kernel.refuse(|op| {
        matches!(op, Op::ExportMesh { .. }).then(|| {
            Error::new(ErrorCode::KernelFailed, "OCCT couldn't mesh it")
                .with_suggestion("export with a coarser tolerance")
        })
    });
    let dir = Scratch::new("kernel");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::KernelFailed, Some("m3tq8v0z")),
        "{e:?}"
    );
    assert!(dir.names().is_empty());
}

#[test]
fn a_mesh_past_a_kernel_requests_time_suggests_a_coarser_tolerance() {
    // M4.1's `export`: a tolerance so fine the mesh takes past a kernel request's 60 s is
    // `worker_timeout`, naming the request, with the design's suggestion.
    let bench = Bench::of(EXAMPLE);
    bench.kernel.refuse(|op| {
        matches!(op, Op::ExportMesh { .. }).then(|| {
            Error::new(
                ErrorCode::WorkerTimeout,
                "the kernel worker didn't answer ExportMesh within 60 s",
            )
            .with_suggestion("the worker's own")
        })
    });
    let dir = Scratch::new("timeout");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.feature(), e.suggestion()),
        (
            ErrorCode::WorkerTimeout,
            Some("m3tq8v0z"),
            Some(
                "export with a coarser tolerance: a finer one makes a larger mesh, which takes longer"
            )
        ),
        "{e:?}"
    );
    assert!(e.message().contains("ExportMesh"), "{e:?}");
    assert!(dir.names().is_empty());
}

#[test]
fn a_model_with_no_bodies_is_refused() {
    // A document of no features: nothing to write (as FreeCAD refuses an export with nothing
    // to mesh).
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": []}"#;
    let bench = Bench::of(text);
    let dir = Scratch::new("empty");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    assert_eq!(problems.errors()[0].code(), ErrorCode::InvalidInput);
    assert!(dir.names().is_empty());
}

#[test]
fn every_body_of_a_model_whose_features_didnt_all_build_is_refused_each_failure_listed() {
    // The cut fails, so it consumes nothing: the block and the pin it would have cut are
    // bodies, as if it were suppressed, which isn't the part the document describes (a pin
    // the cut was to remove, written as an object of its own). Every body asked for is
    // refused, saying so, then the cut's failure as `check` gives it; nothing meshed or
    // written. The block, listed, is exported as it built.
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.refuse(|op| {
            matches!(op, Op::Boolean { .. })
                .then(|| Error::new(ErrorCode::KernelFailed, "the cut failed"))
        });
    });
    let check = command::check(&bench.model);
    assert_eq!(check.total(), 1, "{check:?}");
    assert_eq!(bench.model.bodies(), [id("k7f2qx3m"), id("a91cw0de")]);
    let dir = Scratch::new("failed");
    let path = dir.0.join("out.3mf");
    let problems = refused(bench.export(&request(&path)));
    let lead = &problems.errors()[0];
    assert_eq!(
        (
            lead.code(),
            lead.message(),
            lead.parameter(),
            lead.suggestion()
        ),
        (
            ErrorCode::InvalidInput,
            "every body was asked for, but some of the document's features didn't build, so \
             the bodies aren't the ones it describes",
            Some("features"),
            Some(
                "mend each feature that didn't build (each is listed after this), or list the \
                 features to export"
            ),
        )
    );
    assert_eq!(&problems.errors()[1..], check.errors());
    assert_eq!(problems.total(), 2);
    assert_eq!(bench.meshed(), 0);
    assert!(dir.names().is_empty());
    let mut asked = request(&path);
    asked.features = Some(vec![id("k7f2qx3m")]);
    let done = bench.export(&asked).unwrap();
    assert_eq!(done.objects.len(), 1);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        archive_of(&[("Block", "k7f2qx3m")])
    );
}

#[test]
fn a_feature_listed_after_the_listing_is_full_is_still_counted() {
    // A list of the cut and then the same unknown ID 300 times: every repeat and the unknown
    // one counted, the first hundred listed (the gate's rule), each in the list's order.
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("long-list");
    let mut asked = request(&dir.0.join("out.3mf"));
    let mut listed = vec![id("m3tq8v0z")];
    listed.extend(std::iter::repeat_n(id("zzzzzzzz"), 300));
    asked.features = Some(listed);
    let problems = refused(bench.export(&asked));
    assert_eq!(problems.total(), 300);
    assert_eq!(
        problems.errors().len(),
        silico_core::document::PROBLEMS_LISTED
    );
    let first = &problems.errors()[0];
    assert_eq!(
        (first.message(), first.parameter()),
        ("the document has no feature zzzzzzzz", Some("features.1"))
    );
    let second = &problems.errors()[1];
    assert_eq!(
        (second.message(), second.parameter()),
        (
            "features.2, zzzzzzzz, is listed already",
            Some("features.2")
        )
    );
    assert_eq!(bench.meshed(), 0);
}

/// The name the feature `feature`'s element map gives its face `n`, as an agent reads it.
fn face_name(bench: &Bench, feature: &str, n: u32) -> String {
    let name = bench
        .model
        .element_map(&id(feature))
        .unwrap()
        .name(silico_protocol::ElementRef::face(n))
        .unwrap()
        .to_string();
    // Its own, not a bare number no document holds.
    assert_ne!(name, format!("Face{n}"));
    name
}

/// The export meshes asked of the kernel forced to each face's deflection: the unraised.
fn unraised_asked(bench: &Bench) -> usize {
    bench
        .kernel
        .calls()
        .iter()
        .filter(|c| {
            matches!(
                c.op,
                Op::ExportMesh {
                    force_face_deflection: true,
                    ..
                }
            )
        })
        .count()
}

/// The tetrahedron with its Face4 raised (a tolerance of 0.06 mm, twice it past the 0.1 mm
/// asked), and its unraised mesh as `unraised` makes it of the tetrahedron.
fn raised_bench(unraised: impl Fn(&mut silico_protocol::ExportMesh)) -> Bench {
    let mut raised = tetra();
    raised.face_tolerances[3] = 0.06;
    let mut again = raised.clone();
    again.force_face_deflection = true;
    unraised(&mut again);
    Bench::with(EXAMPLE, move |kernel| {
        kernel.answer_instead(move |op| match op {
            Op::ExportMesh {
                force_face_deflection: false,
                ..
            } => Some(Reply::ExportMesh(Box::new(raised.clone()))),
            Op::ExportMesh {
                force_face_deflection: true,
                ..
            } => Some(Reply::ExportMesh(Box::new(again.clone()))),
            _ => None,
        });
    })
}

#[test]
fn a_raised_face_is_meshed_again_unraised_and_its_first_mesh_written_if_its_nodes_stay() {
    // M4.1's `export` (2b): a face OCCT meshed coarser for its own tolerance is meshed
    // again with `ForceFaceDeflection`, asked of the kernel once, and with the same nodes
    // the first mesh is written, as the tetrahedron's archive.
    let bench = raised_bench(|_| {});
    let dir = Scratch::new("raised-kept");
    let path = dir.0.join("out.3mf");
    bench.export(&request(&path)).unwrap();
    assert_eq!(unraised_asked(&bench), 1);
    assert_eq!(bench.meshed(), 2);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        archive_of(&[("Hole", "m3tq8v0z")])
    );
}

#[test]
fn a_raised_face_whose_nodes_move_unraised_is_refused_by_its_name_and_nothing_written() {
    // Face4's first node moved an ulp unraised: refused, naming the cut, its Face4 by the
    // cut's element map, and the tolerance, at twice the face's: 0.12 mm.
    let bench = raised_bench(|again| {
        let face4 = again.face_ranges()[3].start as usize;
        again.nodes[face4][0] = again.nodes[face4][0].next_up();
    });
    let dir = Scratch::new("raised-refused");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.feature(), e.parameter()),
        (
            ErrorCode::InvalidResult,
            Some("m3tq8v0z"),
            Some("tolerance")
        ),
        "{e:?}"
    );
    assert_eq!(e.elements(), [face_name(&bench, "m3tq8v0z", 4)], "{e:?}");
    assert!(e.message().contains("at 0.12 mm"), "{e:?}");
    assert_eq!(unraised_asked(&bench), 1);
    assert!(dir.names().is_empty());
}

#[test]
fn a_refused_mesh_names_its_faces_by_the_features_element_map() {
    // Face4 meshed with no triangles: the mesh isn't closed, and the face is named as the
    // cut's element map names it, so an agent can find it.
    let mut open = tetra();
    open.triangles.pop();
    open.face_triangles[3] = 0;
    let bench = Bench::with(EXAMPLE, move |kernel| {
        kernel.answer_instead(move |op| match op {
            Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(open.clone()))),
            _ => None,
        });
    });
    let dir = Scratch::new("named");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    let e = &problems.errors()[0];
    assert_eq!(e.elements(), [face_name(&bench, "m3tq8v0z", 4)], "{e:?}");
    assert!(dir.names().is_empty());
}

#[test]
fn faces_bounding_no_material_are_left_out_and_named_by_the_features_element_map() {
    // The block's mesh with a shell of two INTERNAL faces (Face5 and Face6, as OCCT's
    // general fuse makes of a face inside a solid): left out, named in `left_out` by the
    // block's element map, and the tetrahedron written whole.
    let mut membrane = tetra();
    membrane.shell_solids.push(0);
    membrane.shell_outer.push(false);
    membrane.unoriented_faces = vec![5, 6];
    membrane.unoriented_shells = vec![1, 1];
    membrane.validate().unwrap();
    let bench = Bench::with(EXAMPLE, move |kernel| {
        kernel.answer_instead(move |op| match op {
            Op::ExportMesh { .. } => Some(Reply::ExportMesh(Box::new(membrane.clone()))),
            _ => None,
        });
    });
    let dir = Scratch::new("left-out");
    let path = dir.0.join("out.3mf");
    let mut asked = request(&path);
    asked.features = Some(vec![id("k7f2qx3m")]);
    let done = bench.export(&asked).unwrap();
    assert_eq!(
        done.objects[0].left_out,
        [
            face_name(&bench, "k7f2qx3m", 5),
            face_name(&bench, "k7f2qx3m", 6)
        ]
    );
    assert_eq!(done.objects[0].mesh.map(|m| m.triangles), Some(4));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        archive_of(&[("Block", "k7f2qx3m")])
    );
}

#[test]
fn every_body_is_written_in_the_timelines_order() {
    // Two boxes, neither consuming the other: both bodies, written as the timeline has them.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "b2b2b2b2", "type": "box", "v": 1, "name": "Other", "position": [100, 0, 0], "length": 1, "width": 2, "height": 3}]}"#;
    let bench = Bench::of(text);
    let dir = Scratch::new("timeline");
    let path = dir.0.join("out.3mf");
    let done = bench.export(&request(&path)).unwrap();
    let ids: Vec<&str> = done.objects.iter().map(|o| o.feature.as_str()).collect();
    assert_eq!(ids, ["k7f2qx3m", "b2b2b2b2"]);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        archive_of(&[("Block", "k7f2qx3m"), ("Other", "b2b2b2b2")])
    );
}

/// The example, its cut's worker crashed: a kernel fault, no problem of the document's.
fn crashed_cut(text: &str) -> Bench {
    Bench::with(text, |kernel| {
        kernel.refuse(|op| {
            matches!(op, Op::Boolean { .. })
                .then(|| Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
        });
    })
}

/// `e` is the crashed worker's fault, worded as a session keeps it: with no advice of the
/// worker's own, a crash that met a feature says to check with fresh, then to report the
/// document if it crashes the worker again (M4.1 §6, as built in M4.3).
fn is_the_crash(e: &Error) {
    assert_eq!(
        (e.code(), e.message()),
        (ErrorCode::WorkerCrashed, "the kernel worker crashed"),
        "{e:?}"
    );
    assert_eq!(
        e.suggestion(),
        Some(
            "check with fresh to try it again; if the same feature crashes the worker again, \
             report it with the document"
        ),
        "{e:?}"
    );
}

#[test]
fn every_body_of_a_model_only_a_kernel_fault_stopped_is_refused_with_the_fault_alone() {
    // Nothing of the document's needs mending (`check` sets the fault aside), so the
    // refusal is the fault alone, as a read of a solid it stopped is.
    let bench = crashed_cut(EXAMPLE);
    assert_eq!(command::check(&bench.model).total(), 0);
    let dir = Scratch::new("fault");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    assert_eq!(problems.total(), 1, "{problems:?}");
    is_the_crash(&problems.errors()[0]);
    assert_eq!(bench.meshed(), 0);
    assert!(dir.names().is_empty());
}

#[test]
fn a_kernel_fault_beside_failures_past_the_listing_is_listed_straight_after_the_lead() {
    // The cut's worker crashed, and a fillet of another box has 150 broken edge names: the
    // lead, the fault, then the first 98 broken names, every one counted (1 + 1 + 150).
    let edges: Vec<String> = (13..163)
        .map(|n| format!("\"b0000002.box.Edge{n}\""))
        .collect();
    let text = EXAMPLE.replacen(
        "\"note\": \"Clearance for an M5 bolt\"}",
        &format!(
            "\"note\": \"Clearance for an M5 bolt\"}},\n    {{\"id\": \"b0000002\", \"type\": \"box\", \"v\": 1, \"name\": \"Other\", \"position\": [100, 0, 0], \"length\": 1, \"width\": 2, \"height\": 3}},\n    {{\"id\": \"f0000001\", \"type\": \"fillet\", \"v\": 1, \"name\": \"Round\", \"base\": \"b0000002\", \"edges\": [{}], \"radius\": 1}}",
            edges.join(", ")
        ),
        1,
    );
    assert_ne!(text, EXAMPLE);
    let bench = crashed_cut(&text);
    let dir = Scratch::new("fault-and-more");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    assert_eq!(problems.total(), 152, "{:?}", &problems.errors()[..2]);
    assert_eq!(problems.errors()[0].parameter(), Some("features"));
    is_the_crash(&problems.errors()[1]);
    assert!(
        problems.errors()[2..]
            .iter()
            .all(|e| e.code() == ErrorCode::ReferenceBroken)
    );
}

#[test]
fn one_kernel_fault_stopping_several_features_listed_is_said_once() {
    // The pin's and the block's solids are made by the kernel too: with every request
    // refused as a worker that can't be had, the three listed are each stopped by it, and
    // it's said once.
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.refuse(|_| {
            Some(Error::new(
                ErrorCode::WorkerUnavailable,
                "the kernel worker can't be had",
            ))
        });
    });
    let dir = Scratch::new("fault-once");
    let mut asked = request(&dir.0.join("out.3mf"));
    asked.features = Some(vec![id("k7f2qx3m"), id("a91cw0de"), id("m3tq8v0z")]);
    let problems = refused(bench.export(&asked));
    assert_eq!(problems.total(), 1, "{problems:?}");
    assert_eq!(problems.errors()[0].code(), ErrorCode::WorkerUnavailable);
}

#[test]
fn every_body_of_a_model_with_more_failures_than_listed_counts_every_one() {
    // A fillet of 150 broken edge names (a box has 12 edges): the refusal, then the first
    // 99 of them listed (a hundred in all), every one counted: 151. And the fillet listed.
    let edges: Vec<String> = (13..163)
        .map(|n| format!("\"k7f2qx3m.box.Edge{n}\""))
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [
{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}},
{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": [{}], "radius": 1}}]}}"#,
        edges.join(", ")
    );
    let bench = Bench::of(&text);
    let dir = Scratch::new("unlisted");
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    assert_eq!(problems.total(), 151);
    assert_eq!(
        problems.errors().len(),
        silico_core::document::PROBLEMS_LISTED
    );
    assert!(
        problems.errors()[1..]
            .iter()
            .all(|e| e.code() == ErrorCode::ReferenceBroken)
    );
    assert_eq!(bench.meshed(), 0);
    // The fillet listed: its own 150, the first hundred listed, every one counted.
    let mut asked = request(&dir.0.join("out.3mf"));
    asked.features = Some(vec![id("f0000001")]);
    let problems = refused(bench.export(&asked));
    assert_eq!(problems.total(), 150);
    assert_eq!(
        problems.errors().len(),
        silico_core::document::PROBLEMS_LISTED
    );
    assert!(
        problems
            .errors()
            .iter()
            .all(|e| e.code() == ErrorCode::ReferenceBroken)
    );
}

#[test]
fn more_bodies_than_are_meshed_at_once_are_each_written_in_the_timelines_order() {
    // Three more boxes than the machine runs at once (as many as an export once asked of the
    // kernel ahead, each mesh now asked in turn): every box written, in order, each meshed
    // once.
    let count = std::thread::available_parallelism().map_or(1, usize::from) + 3;
    let ids: Vec<String> = (0..count).map(|n| format!("b{n:07}")).collect();
    let features: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(n, id)| {
            format!(
                r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Box{n}", "position": [{}, 0, 0], "length": 1, "width": 1, "height": 1}}"#,
                n * 10
            )
        })
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    let bench = Bench::of(&text);
    let dir = Scratch::new("window");
    let path = dir.0.join("out.3mf");
    let done = bench.export(&request(&path)).unwrap();
    let written: Vec<&str> = done.objects.iter().map(|o| o.feature.as_str()).collect();
    assert_eq!(written, ids);
    assert_eq!(bench.meshed(), count);
    let names: Vec<(String, &str)> = ids
        .iter()
        .enumerate()
        .map(|(n, id)| (format!("Box{n}"), id.as_str()))
        .collect();
    let names: Vec<(&str, &str)> = names.iter().map(|(n, id)| (n.as_str(), *id)).collect();
    assert_eq!(std::fs::read(&path).unwrap(), archive_of(&names));
}

#[test]
fn an_exports_meshes_are_asked_of_the_kernel_one_at_a_time() {
    // A fine tolerance's mesh takes OCCT tens of seconds and gigabytes: one asked at a time,
    // never one for each core. Two bodies, a kernel that runs two requests at once, the
    // first mesh held: the second isn't asked, let alone run, until the first is done.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "b2b2b2b2", "type": "box", "v": 1, "name": "Other", "position": [100, 0, 0], "length": 1, "width": 2, "height": 3}]}"#;
    let bench = Arc::new(Bench::of(text));
    let hold = bench.kernel.hold(|op| matches!(op, Op::ExportMesh { .. }));
    let dir = Scratch::new("one-at-a-time");
    let path = dir.0.join("out.3mf");
    let exporting = {
        let bench = Arc::clone(&bench);
        let path = path.clone();
        std::thread::spawn(move || bench.export(&request(&path)))
    };
    assert!(
        bench
            .kernel
            .wait_running(1, std::time::Duration::from_secs(60))
    );
    // Long enough for a mesh asked ahead to start on the kernel's other thread.
    assert!(
        !bench
            .kernel
            .wait_running(2, std::time::Duration::from_millis(500))
    );
    assert_eq!(bench.asked_meshes(), 1);
    hold.release();
    let done = exporting.join().unwrap().unwrap();
    let ids: Vec<&str> = done.objects.iter().map(|o| o.feature.as_str()).collect();
    assert_eq!(ids, ["k7f2qx3m", "b2b2b2b2"]);
    assert_eq!(bench.meshed(), 2);
}

/// Two boxes, two bodies: "Block" (k7f2qx3m) and "Other" (b2b2b2b2).
const TWO_BOXES: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "b2b2b2b2", "type": "box", "v": 1, "name": "Other", "position": [100, 0, 0], "length": 1, "width": 2, "height": 3}]}"#;

#[test]
fn two_bodies_stopped_after_the_first_writes_no_partial_file() {
    // The session closes while the second body waits its turn to mesh: the export answers
    // the session closed, naming `session`, and writes nothing, never a file of the first
    // body alone (decision 34).
    let bench = Bench::of(TWO_BOXES);
    let dir = Scratch::new("partial");
    let path = dir.0.join("out.3mf");
    // Each wait for a turn looks once before it takes a free one: open at the first body's
    // look, closed from the second's on.
    let looks = std::sync::atomic::AtomicUsize::new(0);
    let result = export(
        &bench.model,
        &request(&path),
        &ExportOn {
            kernel: &bench.kernel,
            step: &*bench.kernel,
            document: "part.silico",
            workspace: None,
            held: &[],
            turns: &silico_core::command::export::MeshTurns::new(),
            stopped: &|| looks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1,
            settle: &|| Ok(()),
        },
    );
    assert!(!path.exists(), "a partial file was written: {result:?}");
    let problems = refused(result);
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (ErrorCode::InvalidInput, Some("session"), None),
        "{e:?}"
    );
    // The first body meshed, the second never asked.
    assert_eq!(bench.asked_meshes(), 1);
    assert!(dir.names().is_empty());
}

#[test]
fn a_mesh_superseded_by_the_sessions_close_is_the_sessions_not_the_solids() {
    // The mesh held, the kernel's generation moved on (as a session's close moves its read's
    // client on), the mesh let go: superseded, so the export answers the session closed,
    // naming `session` and no feature, and writes nothing.
    let bench = Arc::new(Bench::of(EXAMPLE));
    let hold = bench.kernel.hold(|op| matches!(op, Op::ExportMesh { .. }));
    let dir = Scratch::new("superseded");
    let path = dir.0.join("out.3mf");
    let exporting = {
        let bench = Arc::clone(&bench);
        let path = path.clone();
        std::thread::spawn(move || bench.export(&request(&path)))
    };
    assert!(
        bench
            .kernel
            .wait_running(1, std::time::Duration::from_secs(60))
    );
    use silico_core::kernel::Kernel;
    let next = silico_protocol::Generation(bench.kernel.generation().0 + 1);
    bench.kernel.advance(next);
    hold.release();
    let problems = refused(exporting.join().unwrap());
    let e = &problems.errors()[0];
    assert_eq!(
        (e.message(), e.parameter(), e.feature()),
        (
            "the session closed before the export finished",
            Some("session"),
            None
        ),
        "{e:?}"
    );
    assert!(dir.names().is_empty());
}

#[test]
fn two_kernel_faults_stopping_features_listed_are_each_said_once() {
    // The block's worker crashed and the pin's timed out: two faults, each said once.
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.refuse(|op| match op {
            Op::MakePrimitive(silico_protocol::Primitive::Box { .. }) => Some(Error::new(
                ErrorCode::WorkerCrashed,
                "the kernel worker crashed",
            )),
            Op::MakePrimitive(silico_protocol::Primitive::Cylinder { .. }) => Some(Error::new(
                ErrorCode::WorkerTimeout,
                "the kernel worker didn't answer MakePrimitive within 60 s",
            )),
            _ => None,
        });
    });
    let dir = Scratch::new("two-faults");
    let mut asked = request(&dir.0.join("out.3mf"));
    asked.features = Some(vec![id("k7f2qx3m"), id("a91cw0de"), id("m3tq8v0z")]);
    let problems = refused(bench.export(&asked));
    let codes: Vec<ErrorCode> = problems.errors().iter().map(|e| e.code()).collect();
    assert_eq!(
        codes,
        [ErrorCode::WorkerCrashed, ErrorCode::WorkerTimeout],
        "{problems:?}"
    );
    assert_eq!(problems.total(), 2);
}

/// A STEP file's faces or bounds, every one written.
const WHOLE: silico_protocol::StepCount = silico_protocol::StepCount {
    expected: 6,
    written: 6,
};

/// A STEP export of `features` (every body if none) to `path`.
fn step_request(path: &Path, features: Option<&[&str]>) -> Export {
    Export {
        format: ExportFormat::Step,
        path: path.to_owned(),
        replace: false,
        features: features.map(|f| f.iter().map(|&i| id(i)).collect()),
    }
}

#[test]
fn a_step_export_writes_the_kernels_file_of_the_bodies_named_by_their_features() {
    // The fake kernel's file says what was asked: the header's file name, the document's;
    // one product, the cut's, its id the feature's ID and its name the feature's. No mesh,
    // so the object's mesh is none; nothing left out; nothing meshed.
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("step");
    let path = dir.0.join("out.step");
    let done = bench.export(&step_request(&path, None)).unwrap();
    let file = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        file,
        "FAKE STEP\nFILE_NAME('part.silico')\nPRODUCT('m3tq8v0z','Hole') cut(box(40, 20, 10), \
         translate(cylinder(2.5, 12), 20, 10, -1))\n"
    );
    assert_eq!(done.bytes, file.len() as u64);
    assert_eq!(
        done.objects,
        [silico_core::command::export::ExportedObject {
            feature: id("m3tq8v0z"),
            name: "Hole".to_owned(),
            mesh: None,
            left_out: vec![],
        }]
    );
    assert_eq!(bench.meshed(), 0);
    assert_eq!(dir.names(), ["out.step"]);
}

#[test]
fn a_step_export_of_two_bodies_is_an_assembly_named_by_the_documents_stem() {
    // Listed second then first: the products in that order, under `part`, the stem.
    let bench = Bench::of(EXAMPLE);
    let dir = Scratch::new("step-two");
    let path = dir.0.join("out.step");
    bench
        .export(&step_request(&path, Some(&["m3tq8v0z", "k7f2qx3m"])))
        .unwrap();
    let file = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = file.lines().collect();
    assert_eq!(lines[2], "ASSEMBLY('part')");
    assert!(
        lines[3].starts_with("PRODUCT('m3tq8v0z','Hole') "),
        "{file}"
    );
    assert!(
        lines[4].starts_with("PRODUCT('k7f2qx3m','Block') box("),
        "{file}"
    );
    assert_eq!(lines.len(), 5);
}

#[test]
fn a_step_export_encodes_every_name_and_refuses_none_for_its_characters() {
    // The cut renamed with an umlaut, a backslash, and U+0001, which a 3MF can't hold: STEP
    // encodes each (docs/ports/step-export.md, *Strings*), the umlaut and the backslash in
    // one run, so the export is written.
    let renamed = EXAMPLE.replace(r#""name": "Hole""#, r#""name": "Lö\\ch\u0001""#);
    assert_ne!(renamed, EXAMPLE);
    let bench = Bench::of(&renamed);
    let dir = Scratch::new("step-names");
    let path = dir.0.join("out.step");
    let done = bench.export(&step_request(&path, None)).unwrap();
    assert_eq!(done.objects[0].name, "Lö\\ch\u{1}");
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(
        file.contains("PRODUCT('m3tq8v0z','L\\X2\\00F6005C\\X0\\ch\\X2\\0001\\X0\\')"),
        "{file}"
    );
    // The same name, as a 3MF: refused for its control character.
    let problems = refused(bench.export(&request(&dir.0.join("out.3mf"))));
    assert_eq!(problems.errors()[0].parameter(), Some("name"));
}

#[test]
fn a_step_exports_faces_left_out_are_named_by_the_features_element_map() {
    // The writer drops the block's Face5 and Face6 as bounding no material: named in
    // `left_out` as the block's element map names them.
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.answer_instead(|op| match op {
            Op::ExportStep(_) => Some(Reply::StepFile(Box::new(silico_protocol::StepFile {
                step: silico_protocol::Blob::inline(b"STEP".to_vec()),
                left_out: vec![vec![5, 6]],
                unwritten: vec![],
                faces: WHOLE,
                bounds: WHOLE,
            }))),
            _ => None,
        });
    });
    let dir = Scratch::new("step-left-out");
    let done = bench
        .export(&step_request(&dir.0.join("out.step"), Some(&["k7f2qx3m"])))
        .unwrap();
    assert_eq!(
        done.objects[0].left_out,
        [
            face_name(&bench, "k7f2qx3m", 5),
            face_name(&bench, "k7f2qx3m", 6)
        ]
    );
}

#[test]
fn a_step_file_the_kernel_refuses_names_the_feature_of_its_one_part() {
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.refuse(|op| {
            matches!(op, Op::ExportStep(_))
                .then(|| Error::new(ErrorCode::InvalidResult, "OCCT couldn't transfer a face"))
        });
    });
    let dir = Scratch::new("step-refused");
    let problems = refused(bench.export(&step_request(&dir.0.join("out.step"), None)));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::InvalidResult, Some("m3tq8v0z")),
        "{e:?}"
    );
    assert!(dir.names().is_empty());
}

#[test]
fn a_step_reply_for_another_number_of_parts_is_the_workers_fault() {
    // Two parts asked, the faces left out of one sent: a protocol error, nothing written.
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.answer_instead(|op| match op {
            Op::ExportStep(_) => Some(Reply::StepFile(Box::new(silico_protocol::StepFile {
                step: silico_protocol::Blob::inline(b"STEP".to_vec()),
                left_out: vec![vec![]],
                unwritten: vec![],
                faces: WHOLE,
                bounds: WHOLE,
            }))),
            _ => None,
        });
    });
    let dir = Scratch::new("step-parts");
    let problems = refused(bench.export(&step_request(
        &dir.0.join("out.step"),
        Some(&["m3tq8v0z", "k7f2qx3m"]),
    )));
    assert_eq!(problems.errors()[0].code(), ErrorCode::ProtocolError);
    assert!(dir.names().is_empty());
}

#[test]
fn a_step_file_whose_writer_couldnt_transfer_a_face_is_refused_naming_it() {
    // Fable's decision, residual risk: OCCT's writer reports a face it couldn't transfer
    // and goes on. The core refuses, naming the block and its Face3 by the element map,
    // OCCT's words its cause, and writes nothing. A part past the last is the worker's
    // fault.
    let answering = |part: u32| {
        Bench::with(EXAMPLE, move |kernel| {
            kernel.answer_instead(move |op| match op {
                Op::ExportStep(_) => Some(Reply::StepFile(Box::new(silico_protocol::StepFile {
                    step: silico_protocol::Blob::inline(b"STEP".to_vec()),
                    left_out: vec![vec![]],
                    unwritten: vec![silico_protocol::StepUnwritten {
                        part: Some(part),
                        element: Some(silico_protocol::ElementRef::face(3)),
                        messages: vec!["Face not transferred".to_owned()],
                    }],
                    faces: WHOLE,
                    bounds: WHOLE,
                }))),
                _ => None,
            });
        })
    };
    let bench = answering(0);
    let dir = Scratch::new("step-unwritten");
    let path = dir.0.join("out.step");
    let problems = refused(bench.export(&step_request(&path, Some(&["k7f2qx3m"]))));
    assert_eq!(problems.total(), 1);
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.feature(), e.cause()),
        (
            ErrorCode::InvalidResult,
            Some("k7f2qx3m"),
            Some("Face not transferred")
        ),
        "{e:?}"
    );
    assert_eq!(e.elements(), [face_name(&bench, "k7f2qx3m", 3)]);
    assert!(dir.names().is_empty());
    let bench = answering(1);
    let problems = refused(bench.export(&step_request(&path, Some(&["k7f2qx3m"]))));
    assert_eq!(problems.errors()[0].code(), ErrorCode::ProtocolError);
    assert!(dir.names().is_empty());
}

/// The example's bench, its STEP exports answered with `file`.
fn step_answered(file: silico_protocol::StepFile) -> Bench {
    Bench::with(EXAMPLE, move |kernel| {
        kernel.answer_instead(move |op| match op {
            Op::ExportStep(_) => Some(Reply::StepFile(Box::new(file.clone()))),
            _ => None,
        });
    })
}

#[test]
fn a_step_file_short_of_faces_or_bounds_is_refused_saying_how_many_and_whose() {
    // The writer drops a face, or a face's hole, with only a warning: the worker's counts say
    // so. One part: each shortfall names its feature; two: none, the advice to export them
    // one at a time.
    let short = |faces: (u64, u64), bounds: (u64, u64), parts: usize| silico_protocol::StepFile {
        step: silico_protocol::Blob::inline(b"STEP".to_vec()),
        left_out: vec![vec![]; parts],
        unwritten: vec![],
        faces: silico_protocol::StepCount {
            expected: faces.0,
            written: faces.1,
        },
        bounds: silico_protocol::StepCount {
            expected: bounds.0,
            written: bounds.1,
        },
    };
    let bench = step_answered(short((7, 6), (9, 8), 1));
    let dir = Scratch::new("step-short");
    let path = dir.0.join("out.step");
    let problems = refused(bench.export(&step_request(&path, Some(&["k7f2qx3m"]))));
    let said: Vec<(&str, Option<&str>)> = problems
        .errors()
        .iter()
        .map(|e| (e.message(), e.feature()))
        .collect();
    assert_eq!(
        said,
        [
            (
                "the STEP file holds 6 of the 7 faces the bodies have that bound material: \
                 OCCT's writer dropped 1",
                Some("k7f2qx3m")
            ),
            (
                "the STEP file holds 8 of the 9 face bounds the bodies have that bound \
                 material: OCCT's writer dropped 1",
                Some("k7f2qx3m")
            ),
        ]
    );
    assert!(
        problems
            .errors()
            .iter()
            .all(|e| e.code() == ErrorCode::InvalidResult)
    );
    assert!(dir.names().is_empty());
    // More written than the parts have (the worker counts a face two solids share once for
    // each, as the writer writes it): not the parts as they are, refused too.
    let bench = step_answered(short((7, 8), (9, 9), 1));
    let problems = refused(bench.export(&step_request(&path, Some(&["k7f2qx3m"]))));
    assert_eq!(
        problems.errors()[0].message(),
        "the STEP file holds 8 faces where the bodies have 7 that bound material: OCCT's \
         writer didn't write them as they are"
    );
    assert!(dir.names().is_empty());
    // Two parts: whose it is isn't known.
    let bench = step_answered(short((14, 13), (14, 14), 2));
    let problems = refused(bench.export(&step_request(&path, Some(&["m3tq8v0z", "k7f2qx3m"]))));
    let e = &problems.errors()[0];
    assert_eq!(e.feature(), None, "{e:?}");
    assert_eq!(
        e.suggestion(),
        Some("export the bodies one at a time, to find which the writer can't write whole")
    );
}

#[test]
fn a_step_check_list_of_many_elements_and_messages_lists_a_hundred_and_says_every_message() {
    // 101 faces at fault, in two entries with a message each: the first hundred named, all
    // counted, both messages the cause.
    let entries = vec![
        silico_protocol::StepUnwritten {
            part: Some(0),
            element: Some(silico_protocol::ElementRef::face(1)),
            messages: vec!["first".to_owned()],
        },
        silico_protocol::StepUnwritten {
            part: Some(0),
            element: Some(silico_protocol::ElementRef::face(2)),
            messages: vec!["second".to_owned()],
        },
    ];
    let mut many = entries.clone();
    many.extend((3..=101).map(|n| silico_protocol::StepUnwritten {
        part: Some(0),
        element: Some(silico_protocol::ElementRef::face(n)),
        messages: vec![],
    }));
    let bench = step_answered(silico_protocol::StepFile {
        step: silico_protocol::Blob::inline(b"STEP".to_vec()),
        left_out: vec![vec![]],
        unwritten: many,
        faces: WHOLE,
        bounds: WHOLE,
    });
    let dir = Scratch::new("step-many");
    let problems =
        refused(bench.export(&step_request(&dir.0.join("out.step"), Some(&["k7f2qx3m"]))));
    assert_eq!(problems.total(), 1);
    let e = &problems.errors()[0];
    assert_eq!(e.elements().len(), 100);
    assert_eq!(e.elements_total(), 101);
    assert_eq!(e.cause(), Some("first; second"));
    assert_eq!(e.feature(), Some("k7f2qx3m"));
}

#[test]
fn a_two_part_step_file_the_writer_refuses_names_no_feature() {
    // Whatever it refused isn't known to be either body's: no feature named.
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.refuse(|op| {
            matches!(op, Op::ExportStep(_)).then(|| {
                Error::new(ErrorCode::InvalidResult, "OCCT couldn't write the file")
                    .with_suggestion("the writer's own")
            })
        });
    });
    let dir = Scratch::new("step-two-refused");
    let problems = refused(bench.export(&step_request(
        &dir.0.join("out.step"),
        Some(&["m3tq8v0z", "k7f2qx3m"]),
    )));
    assert_eq!(problems.errors()[0].feature(), None);
    // Its advice its own: only a timeout's is the export's to give.
    assert_eq!(problems.errors()[0].suggestion(), Some("the writer's own"));
    assert!(dir.names().is_empty());
}

#[test]
fn a_step_file_past_a_requests_time_suggests_exporting_fewer_features() {
    let bench = Bench::with(EXAMPLE, |kernel| {
        kernel.refuse(|op| {
            matches!(op, Op::ExportStep(_)).then(|| {
                Error::new(
                    ErrorCode::WorkerTimeout,
                    "the STEP worker didn't answer ExportStep within 60 s",
                )
                .with_suggestion("the worker's own")
            })
        });
    });
    let dir = Scratch::new("step-timeout");
    let problems = refused(bench.export(&step_request(&dir.0.join("out.step"), None)));
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.feature(), e.suggestion()),
        (
            ErrorCode::WorkerTimeout,
            Some("m3tq8v0z"),
            Some("export fewer features at once: the STEP writer writes every body in one request")
        ),
        "{e:?}"
    );
}
