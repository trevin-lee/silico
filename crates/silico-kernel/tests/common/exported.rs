//! For tests of the export: a shape exported through a kernel worker, as the core exports.
#![allow(dead_code)]

use super::kernel::kernel;

use silico_core::export::{PrintableMesh, needs_unraised, printable};
use silico_core::kernel::ShapeData;
use silico_kernel::{Shape, to_brep};
use silico_protocol::{Blob, ExportMesh};

/// `shape`'s B-rep, as a request carries it.
pub fn data(shape: &Shape) -> ShapeData {
    ShapeData::unchecked(Blob::inline(to_brep(shape).unwrap()))
}

/// `shape` exported through a kernel worker, as the core exports, at `tolerance`.
pub fn exported(shape: &Shape, tolerance: f64) -> Result<PrintableMesh, silico_core::Error> {
    let shape = data(shape);
    let mesh = kernel().export_mesh(&shape, tolerance)?;
    checked(&shape, &mesh)
}

/// `mesh`, `shape`'s export mesh, checked as the core checks it: with OCCT's unraised mesh
/// of `shape` beside it where OCCT raised a face's deflection.
pub fn checked(shape: &ShapeData, mesh: &ExportMesh) -> Result<PrintableMesh, silico_core::Error> {
    let unraised = if needs_unraised(mesh) {
        Some(kernel().unraised_export_mesh(shape, mesh.tolerance)?)
    } else {
        None
    };
    printable(mesh, unraised.as_ref(), None)
}
