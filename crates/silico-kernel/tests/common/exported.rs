//! For tests of the export: a shape exported through a kernel worker, as the core exports.
#![allow(dead_code)]

use super::kernel::kernel;

use silico_core::export::{PrintableMesh, printable};
use silico_core::kernel::ShapeData;
use silico_kernel::{Shape, to_brep};
use silico_protocol::Blob;

/// `shape`'s B-rep, as a request carries it.
pub fn data(shape: &Shape) -> ShapeData {
    ShapeData::unchecked(Blob::inline(to_brep(shape).unwrap()))
}

/// `shape` exported through a kernel worker, as the core exports, at `tolerance`.
pub fn exported(shape: &Shape, tolerance: f64) -> Result<PrintableMesh, silico_core::Error> {
    let mesh = kernel().export_mesh(&data(shape), tolerance)?;
    printable(&mesh, None)
}
