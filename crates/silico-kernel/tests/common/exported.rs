//! For tests of the export: a shape exported through a kernel worker, as the core exports.
#![allow(dead_code)]

use super::kernel::kernel;

use silico_core::export::{PrintableMesh, printable};
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
/// of `shape` to compare with where OCCT raised a face's deflection.
pub fn checked(shape: &ShapeData, mesh: &ExportMesh) -> Result<PrintableMesh, silico_core::Error> {
    printable(
        mesh,
        || kernel().unraised_export_mesh(shape, mesh.tolerance),
        None,
    )
}

/// The unraised mesh, for a report none of whose faces OCCT raised: never asked for.
pub fn unasked() -> Result<ExportMesh, silico_core::Error> {
    panic!("the unraised mesh was asked for, though no face is raised")
}
