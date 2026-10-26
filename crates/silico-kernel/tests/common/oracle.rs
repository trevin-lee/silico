//! The FreeCAD oracle, for tests that ask FreeCAD what the harness's cases can't (history,
//! element names, shapes to measure): a script run as `silico-oracle` runs its cases.
#![allow(dead_code)]

use silico_protocol::{Blob, ShapeData};

/// Runs `script` (Python, with FreeCAD's modules importable) and returns the value it
/// printed as `print("SILICO_ORACLE " + json.dumps(value))`.
pub fn freecad(script: &str) -> serde_json::Value {
    silico_oracle::freecad::script(script).unwrap_or_else(|e| panic!("{e}"))
}

/// The text B-rep of `shape`, as FreeCAD makes it from `body` (Python that sets `shape`,
/// with `json`, `math`, `Part`, `App`, and `V` for `App.Vector` at hand).
pub fn freecad_text(body: &str) -> String {
    let script = format!(
        "import json, math, Part, FreeCAD as App\nV = App.Vector\n{body}\n\
         print(\"SILICO_ORACLE \" + json.dumps(shape.exportBrepToString()))\n"
    );
    freecad(&script).as_str().unwrap().to_owned()
}

/// The shape FreeCAD makes from `body` (Python that sets `shape`), as a kernel worker's.
pub fn freecad_shape(body: &str) -> ShapeData {
    from_freecad(&freecad_text(body))
}

/// The face FreeCAD makes from `body` (Python that sets `face`), checked valid there.
pub fn freecad_face(body: &str) -> ShapeData {
    freecad_shape(&format!("{body}\nassert face.isValid()\nshape = face\n"))
}

/// A shape FreeCAD wrote as a text B-rep, as a kernel worker's shape, its validity OCCT's.
pub fn from_freecad(text: &str) -> ShapeData {
    let shape = silico_kernel::from_text_brep(text.as_bytes()).unwrap();
    let valid = shape.is_valid().unwrap();
    ShapeData::checked(Blob::inline(silico_kernel::to_brep(&shape).unwrap()), valid)
}
