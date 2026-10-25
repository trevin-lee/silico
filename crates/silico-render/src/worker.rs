//! The render worker (ARCHITECTURE.md, Process model; docs/design/m4.6-render.md): the
//! worker loop every worker shares (`silico_worker`), drawing scenes with `re_renderer`. It
//! keeps its render context between renders; stopping the worker is what gives the
//! context's memory back (the core will, after a large render and when it's idle: the
//! design's step 3).

use silico_protocol::{Failure, Op, Refusal, Reply};

use crate::render::Renderer;

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
                    None => {
                        let adapter = crate::adapter(self.policy)
                            .map_err(|cause| Failure::new(Refusal::NoAdapter { cause }))?;
                        tracing::info!(adapter = adapter.get_info().name, "render context");
                        self.renderer
                            .insert(Renderer::new(&adapter).map_err(internal)?)
                    }
                };
                let rendered = renderer.render(&scene).map_err(internal)?;
                Ok(Reply::Rendered {
                    png: rendered.png,
                    ids: rendered.ids,
                })
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

fn internal(what: String) -> Failure {
    Failure::new(Refusal::Internal { what })
}
