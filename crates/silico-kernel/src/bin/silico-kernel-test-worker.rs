//! The kernel worker with the test operations, for tests of the core's worker client:
//! sleeping, aborting, crashing, replying with garbage, switching floating-point traps on,
//! and misbehaving on request. Built only with the `test_ops` feature, and kept in its own
//! binary so the production worker's code never dispatches these.

use silico_kernel::{test_ops, worker};
use silico_protocol::{
    DebugOp, Description, DressingKind, EdgeCurve, EdgeDescription, EdgeTrouble, ElementKind,
    FaceDescription, Failure, Generation, Mesh, Misbehavior, Op, Refusal, Reply, RequestId,
    Surface, Trouble,
};

fn main() -> std::process::ExitCode {
    let mut misbehavior = None;
    worker::run(move |id, generation, op, socket| match op {
        Op::Debug(DebugOp::Misbehave(how)) => {
            misbehavior = Some(how);
            Ok(Reply::Done)
        }
        Op::Debug(_) => debug(id, generation, op, socket),
        Op::Tessellate { .. } if misbehavior == Some(Misbehavior::BadMesh) => {
            Ok(Reply::Mesh(Mesh {
                positions: vec![[0.0; 3]; 3],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                triangles: vec![[0, 1, 3]],
                face_triangles: vec![1],
                edge_points: vec![],
                edge_point_counts: vec![],
                valid: None,
            }))
        }
        _ if misbehavior == Some(Misbehavior::WrongReply) => Ok(Reply::Done),
        Op::Describe {
            kind, first, count, ..
        } if misbehavior == Some(Misbehavior::EndlessDescription) => Ok(Reply::Described {
            total: u32::MAX,
            description: endless(kind, first, count),
        }),
        Op::Fillet { .. } if misbehavior == Some(Misbehavior::ForgedRefusal) => {
            Err(Failure::new(Refusal::EdgesRefused {
                dressing: DressingKind::Fillet,
                troubles: vec![EdgeTrouble {
                    edge: 7,
                    trouble: Trouble::Smooth,
                }],
            }))
        }
        Op::Fillet { .. }
        | Op::Translate { .. }
        | Op::Boolean { .. }
        | Op::Copies { .. }
        | Op::Extrude { .. }
        | Op::Revolve { .. }
        | Op::MakeFaces(_)
        | Op::Prism { .. }
        | Op::PrismThroughAll { .. }
        | Op::Revolution { .. }
            if matches!(
                misbehavior,
                Some(Misbehavior::BadHistory | Misbehavior::HistoryForNoInputs)
            ) =>
        {
            let mut reply = worker::handle(op)?;
            if let Reply::Built(built) = &mut reply {
                if misbehavior == Some(Misbehavior::HistoryForNoInputs) {
                    built.history.inputs.clear();
                } else if let Some(input) = built.history.inputs.first_mut() {
                    // Its first face, or (for faces made from regions, which have none) its
                    // first edge.
                    if let Some(fate) = input.faces.first_mut().or(input.edges.first_mut()) {
                        fate.kept = Some(99);
                    }
                }
            }
            Ok(reply)
        }
        other => worker::handle(other),
    })
}

/// `count` sound elements of `kind` from `first` (each at its own number along x), as
/// [`Misbehavior::EndlessDescription`] sends them.
fn endless(kind: ElementKind, first: u32, count: u32) -> Description {
    let at = |n: u32, y: f64| [f64::from(n), y, 0.0];
    let numbers = first..first.saturating_add(count);
    let mut description = Description {
        faces: Vec::new(),
        edges: Vec::new(),
        vertices: Vec::new(),
    };
    match kind {
        ElementKind::Face => {
            description.faces = numbers
                .map(|n| FaceDescription {
                    surface: Surface::Other,
                    area: 1.0,
                    centroid: at(n, 0.0),
                })
                .collect();
        }
        ElementKind::Edge => {
            description.edges = numbers
                .map(|n| EdgeDescription {
                    curve: EdgeCurve::Other,
                    length: 1.0,
                    start: at(n, 0.0),
                    end: at(n, 1.0),
                    middle: at(n, 0.5),
                })
                .collect();
        }
        ElementKind::Vertex | ElementKind::Solid => {
            description.vertices = numbers.map(|n| at(n, 0.0)).collect();
        }
    }
    description
}

/// The test operations other than `Misbehave`: those every test worker shares, then the
/// ones that go through OCCT.
fn debug(
    id: RequestId,
    generation: Generation,
    op: Op,
    socket: &mut dyn std::io::Write,
) -> Result<Reply, Failure> {
    let Op::Debug(op) = op else {
        return worker::handle(op);
    };
    match silico_worker::test_ops::answer(id, generation, op, socket) {
        Ok(result) => result,
        Err(DebugOp::Segfault) => {
            test_ops::raise_sigsegv();
            Ok(Reply::Done)
        }
        Err(DebugOp::EnableFpTraps) => {
            test_ops::enable_fp_traps().map_err(|e| {
                Failure::new(Refusal::Internal {
                    what: e.message().to_owned(),
                })
            })?;
            Ok(Reply::Done)
        }
        Err(other) => worker::handle(Op::Debug(other)),
    }
}
