//! The sketch worker (ARCHITECTURE.md, Process model): the worker loop every worker shares
//! (`silico_worker`), solving sketches with PlaneGCS. A solve keeps nothing: each request
//! carries the whole sketch. A drag is the one state it keeps ([`State`]): begun with the
//! whole sketch, then moved by the cursor alone, until it ends.

use silico_protocol::{Failure, Op, Refusal, Reply};

use crate::Drag;

/// How the sketch worker reads and switches off floating-point traps: directly, as it
/// links no library that manages them.
pub const PLATFORM: silico_worker::Platform = silico_worker::Platform {
    traps: silico_worker::Traps::Checked {
        mask: crate::fp_trap_mask,
        disable: crate::disable_fp_traps,
    },
    built_from: "PlaneGCS from FreeCAD 1.1.3",
};

/// What the sketch worker keeps between requests: the drag under way, if there is one.
#[derive(Default)]
pub struct State {
    drag: Option<Drag>,
}

impl State {
    /// Whether a drag is under way.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The production operations: a solve, a drag's begin, moves, and end, and the echo
    /// every worker answers. Anything else is another worker's (or a test operation).
    ///
    /// A begin ends any drag under way, and starts one unless constraints conflict; a solve
    /// ends it, as FreeCAD's normal solve ends its drag; an end ends it. A move that fails
    /// because the bridge threw ends it too: nothing vouches for the solver's state after
    /// that. The echo leaves it alone.
    pub fn handle(&mut self, op: Op) -> Result<Reply, Failure> {
        match op {
            Op::Solve(system) => {
                self.drag = None;
                // The core checked it before sending; checked again, since PlaneGCS trusts
                // its input entirely.
                system.check().map_err(crate::invalid)?;
                Ok(Reply::Solved(crate::solve(&system)?))
            }
            Op::DragBegin(begin) => {
                self.drag = None;
                let start = Drag::begin(&begin)?;
                self.drag = start.drag;
                Ok(Reply::DragStarted {
                    dofs: start.dofs,
                    conflicting: start.conflicting,
                })
            }
            Op::DragMove { to } => {
                let Some(drag) = self.drag.as_mut() else {
                    return Err(no_drag(&op));
                };
                match drag.move_to(to) {
                    Ok(moved) => Ok(Reply::Moved(moved)),
                    Err(failure) => {
                        if matches!(failure.refusal, Refusal::SolverThrew | Refusal::OutOfMemory) {
                            self.drag = None;
                        }
                        Err(failure)
                    }
                }
            }
            Op::DragEnd => match self.drag.take() {
                Some(_) => Ok(Reply::Done),
                None => Err(no_drag(&op)),
            },
            Op::Echo { data } => Ok(Reply::Echo { data }),
            Op::Debug(_) => Err(Failure::new(Refusal::TestOperation {
                op: op.name().to_owned(),
            })),
            other => Err(Failure::new(Refusal::OtherWorkersOp {
                op: other.name().to_owned(),
            })),
        }
    }
}

fn no_drag(op: &Op) -> Failure {
    Failure::new(Refusal::NoDragSession {
        op: op.name().to_owned(),
    })
}
