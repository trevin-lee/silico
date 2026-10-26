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
                workspace: None,
                held: &[],
                settle,
            },
        )
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
        format: ExportFormat::ThreeMf,
        path: path.to_owned(),
        replace: false,
        features: None,
        tolerance: Length::rounded(0.1).unwrap(),
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
                triangles: 4,
                vertices: 4,
                volume: Measure::parse("0.166667").unwrap(),
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
    asked.tolerance = Length::rounded(0.0).unwrap();
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
    asked.tolerance = Length::rounded(1e9).unwrap();
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
    assert_eq!(done.objects[0].triangles, 4);
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

/// `e` is the crashed worker's fault, worded as a read of the revision keeps it (reading
/// it again won't try again).
fn is_the_crash(e: &Error) {
    assert_eq!(
        (e.code(), e.message()),
        (ErrorCode::WorkerCrashed, "the kernel worker crashed"),
        "{e:?}"
    );
    assert!(
        e.suggestion()
            .is_some_and(|s| s.contains("reading this revision again won't")),
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
    // The export asks as many meshes ahead as the machine runs at once: three more boxes than
    // that, so the window is refilled past its first fill. Every box written, in order, each
    // meshed once.
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
