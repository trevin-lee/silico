//! Reads an OCCT text B-rep (as FreeCAD's `Shape.exportBrep` writes it) on stdin, and
//! writes it as Silico's B-rep on stdout: how the FreeCAD oracle's corpus of real parts
//! (`pixi run corpus`) reaches Silico. A shape Silico would refuse is refused here too.

use std::io::{Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut text = Vec::new();
    let converted = std::io::stdin()
        .read_to_end(&mut text)
        .map_err(|e| format!("can't read stdin: {e}"))
        .and_then(|_| silico_kernel::from_text_brep(&text).map_err(|e| e.to_string()))
        .and_then(|shape| silico_kernel::to_brep(&shape).map_err(|e| e.to_string()))
        .and_then(|brep| {
            std::io::stdout()
                .write_all(&brep)
                .map_err(|e| format!("can't write stdout: {e}"))
        });
    match converted {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "silico-brep-convert: {e}");
            ExitCode::FAILURE
        }
    }
}
