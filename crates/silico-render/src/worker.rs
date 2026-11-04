//! The render worker (ARCHITECTURE.md, Process model; docs/design/m4.6-render.md): the
//! worker loop every worker shares (`silico_worker`), drawing scenes with `re_renderer`. It
//! keeps its render context between renders, on a device of its own, and with it the last
//! scene's bodies uploaded, so another view of them uploads nothing; stopping the worker is
//! what gives the context's memory back (the core will, after a large render and when it's idle: the
//! design's step 3).

use silico_protocol::{Failure, Op, Refusal, Reply};

use crate::render::{self, RenderFailure, Renderer};

/// The render worker's platform: it has no C++ to read floating-point traps through, and
/// reading them from Rust takes inline assembly, so they're unchecked, as data (R5).
pub const PLATFORM: silico_worker::Platform = silico_worker::Platform {
    traps: silico_worker::Traps::Unchecked,
    built_from: "re_renderer 0.38.1 on wgpu 30",
};

pub use silico_protocol::RenderAdapter;

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
                    None => self.renderer.insert(render_context(self.policy)?),
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
                        Err(refused(failure))
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

/// A failed render as the worker refuses it: out of memory as a shortage
/// (`resource_exhausted`, its words the cause), anything else as the renderer's own fault.
fn refused(failure: RenderFailure) -> Failure {
    match failure {
        RenderFailure::OutOfMemory(cause) => Failure {
            refusal: Refusal::OutOfMemory,
            cause: Some(cause),
        },
        // A render's scene passed its check, so this is a frame's or an upload's, never
        // the worker's; refused as the check refuses, all the same.
        RenderFailure::Invalid(invalid) => Failure::new(Refusal::InvalidParameter {
            parameter: invalid.parameter.to_owned(),
            message: invalid.message,
        }),
        RenderFailure::Failed(what) => internal(what),
        // A frame's, never a render's: the worker's renders don't back off.
        failure @ RenderFailure::BackingOff { .. } => internal(failure.to_string()),
    }
}

/// A render context on the first adapter `policy` allows that can make one, or why none
/// can: a fault of the machine, not of the scene.
fn render_context(policy: RenderAdapter) -> Result<Renderer, Failure> {
    let unavailable = |cause| Failure::new(Refusal::NoAdapter { cause });
    let mut tried = Vec::new();
    for adapter in crate::adapters(policy).map_err(unavailable)? {
        let name = adapter.get_info().name;
        let made = render::device(&adapter).and_then(|(device, queue)| {
            Renderer::new(&adapter, device, queue, render::OUTPUT_FORMAT)
        });
        match made {
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

#[cfg(test)]
mod tests {
    use silico_protocol::{FailureKind, Refusal};

    use super::refused;
    use crate::render::RenderFailure;

    #[test]
    fn a_render_out_of_memory_is_a_shortage_and_anything_else_the_renderers_fault() {
        let short = refused(RenderFailure::OutOfMemory("4 GiB asked".to_owned()));
        assert_eq!(short.refusal, Refusal::OutOfMemory);
        assert_eq!(short.cause.as_deref(), Some("4 GiB asked"));
        assert_eq!(short.kind(), FailureKind::ResourceExhausted);
        let failed = refused(RenderFailure::Failed("a readback never came".to_owned()));
        assert_eq!(
            failed.refusal,
            Refusal::Internal {
                what: "a readback never came".to_owned()
            }
        );
        assert_eq!(failed.kind(), FailureKind::Internal);
    }
}
