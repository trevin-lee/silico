//! The core's commands on a document's bytes, as a client without a live model runs them:
//! through the gate, rebuilt on a fake kernel, then the command.

use silico_core::Error;
use silico_core::command::{self, Formatted, Problems};
use silico_core::document::Checked;
use silico_core::fake::FakeKernel;
use silico_core::kernel::Kernel;
use silico_core::recompute::{Model, Recompute};

pub fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap())
}

fn rebuilt<K: Kernel + 'static>(
    engine: &Recompute<K>,
    checked: Checked,
) -> Result<std::sync::Arc<Model>, Error> {
    Ok(command::rebuild(engine, checked)?.expect("nothing else uses the engine"))
}

/// `command::check` of a document's bytes: the gate's problems, or the rebuild's.
pub fn check_bytes<K: Kernel + 'static>(
    engine: &Recompute<K>,
    bytes: &[u8],
) -> Result<Problems, Error> {
    match command::gate(bytes) {
        Ok(checked) => command::check(&*rebuilt(engine, checked)?),
        Err(problems) => Ok(problems),
    }
}

/// `command::format` of a document's bytes: the gate's problems, or the formatted text.
pub fn format_bytes<K: Kernel + 'static>(
    engine: &Recompute<K>,
    bytes: &[u8],
) -> Result<Result<Formatted, Problems>, Error> {
    match command::gate(bytes) {
        Ok(checked) => command::format(&*rebuilt(engine, checked)?),
        Err(problems) => Ok(Err(problems)),
    }
}
