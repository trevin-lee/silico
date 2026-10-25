//! The render worker (ARCHITECTURE.md, Process model; docs/design/m4.6-render.md): the
//! worker loop every worker shares (`silico_worker`), drawing scenes with `re_renderer`. It
//! keeps its render context between renders; stopping the worker is what gives the
//! context's memory back (the core will, after a large render and when it's idle: the
//! design's step 3).

use silico_protocol::{Failure, Op, Refusal, Reply};

use crate::render::{RenderFailure, Renderer};

/// The render worker's platform: it has no C++ to read floating-point traps through, and
/// reading them from Rust takes inline assembly, so they're unchecked, as data (R5).
pub const PLATFORM: silico_worker::Platform = silico_worker::Platform {
    traps: silico_worker::Traps::Unchecked,
    built_from: "re_renderer 0.38.1 on wgpu 30",
};

/// Which adapter the worker draws on: the core's one say, given as an argument
/// (docs/design/m4.6-render.md, *How the worker gets its rasterizer*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderAdapter {
    /// Only a CPU adapter (lavapipe), or none: the default.
    Software,
    /// A CPU adapter if there is one, then a discrete GPU, then any.
    Any,
}

/// What the render worker keeps between requests: its render context, made on the first
/// render.
pub struct State {
    policy: RenderAdapter,
    renderer: Option<Renderer>,
}

impl State {
    pub fn new(policy: RenderAdapter) -> State {
        State {
            policy,
            renderer: None,
        }
    }

    /// The production operations: a render, and the echo every worker answers. Anything
    /// else is another worker's (or a test operation).
    pub fn handle(&mut self, op: Op) -> Result<Reply, Failure> {
        match op {
            Op::Render(scene) => {
                // The core checked it before sending; checked again, so a hand-built
                // request can't index out of range.
                scene.check().map_err(|invalid| {
                    Failure::new(Refusal::InvalidParameter {
                        parameter: invalid.parameter.to_owned(),
                        message: invalid.message,
                    })
                })?;
                let renderer = match &mut self.renderer {
                    Some(renderer) => renderer,
                    None => self.renderer.insert(renderer(self.policy)?),
                };
                match renderer.render(&scene) {
                    Ok(rendered) => Ok(Reply::Rendered {
                        png: rendered.png,
                        ids: rendered.ids,
                    }),
                    Err(failure) => {
                        // Nothing vouches for the context after a failure: the next render
                        // makes a new one.
                        self.renderer = None;
                        Err(match failure {
                            RenderFailure::OutOfMemory(cause) => Failure {
                                refusal: Refusal::OutOfMemory,
                                cause: Some(cause),
                            },
                            RenderFailure::Failed(what) => internal(what),
                        })
                    }
                }
            }
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

/// A render context on the first adapter `policy` allows that can make one, or why none
/// can: a fault of the machine, not of the scene.
fn renderer(policy: RenderAdapter) -> Result<Renderer, Failure> {
    let unavailable = |cause| Failure::new(Refusal::NoAdapter { cause });
    let mut tried = Vec::new();
    for adapter in crate::adapters(policy).map_err(unavailable)? {
        let name = adapter.get_info().name;
        match Renderer::new(&adapter) {
            Ok(renderer) => {
                tracing::info!(adapter = name, "render context");
                return Ok(renderer);
            }
            Err(why) => tried.push(format!("{name}: {why}")),
        }
    }
    Err(unavailable(format!(
        "no adapter could make a render context ({})",
        tried.join("; ")
    )))
}

fn internal(what: String) -> Failure {
    Failure::new(Refusal::Internal { what })
}
