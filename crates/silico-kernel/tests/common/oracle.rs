//! The FreeCAD oracle, for tests that ask FreeCAD what the harness's cases can't (history,
//! element names): a script run as `silico-oracle` runs its cases.

/// Runs `script` (Python, with FreeCAD's modules importable) and returns the value it
/// printed as `print("SILICO_ORACLE " + json.dumps(value))`.
pub fn freecad(script: &str) -> serde_json::Value {
    silico_oracle::freecad::script(script).unwrap_or_else(|e| panic!("{e}"))
}
