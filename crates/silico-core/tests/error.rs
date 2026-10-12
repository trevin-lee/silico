//! M0.5: the structured error. Its JSON form is the contract with agents, so it's checked
//! against committed files (tests/contract/), and changing it means changing them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::{Error, ErrorCode};

fn contract(name: &str) -> String {
    let path = format!("{}/tests/contract/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn full() -> Error {
    Error::new(ErrorCode::KernelFailed, "fillet failed")
        .with_feature("Fillet1")
        .with_element("Edge3")
        .with_element("Edge5")
        .with_parameter("radius")
        .with_cause("StdFail_NotDone: BRepFilletAPI_MakeFillet")
        .with_suggestion("reduce the radius below 5 mm")
        .with_candidates(["Edge2", "Edge4"])
}

#[test]
fn error_json_shape() {
    let json = serde_json::to_string_pretty(&full()).unwrap();
    assert_eq!(json + "\n", contract("error-full.json"));
}

#[test]
fn error_json_shape_without_optional_fields() {
    // Absent fields are present as null (and candidates as []), so agents see one shape.
    let error = Error::new(ErrorCode::Internal, "unexpected state");
    let json = serde_json::to_string_pretty(&error).unwrap();
    assert_eq!(json + "\n", contract("error-minimal.json"));
}

#[test]
fn error_codes_are_stable() {
    let expected: Vec<String> = contract("error-codes.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(str::to_owned)
        .collect();
    let actual: Vec<String> = ErrorCode::ALL
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();
    assert_eq!(actual, expected);
    // The string form and the serialized form are the same thing.
    for code in ErrorCode::ALL {
        assert_eq!(
            serde_json::to_value(code).unwrap(),
            serde_json::Value::String(code.as_str().to_owned())
        );
    }
}

#[test]
fn display_names_feature_elements_and_parameter() {
    assert_eq!(
        full().to_string(),
        "kernel_failed: fillet failed (feature Fillet1, elements Edge3, Edge5, parameter radius): \
         StdFail_NotDone: BRepFilletAPI_MakeFillet. Suggestion: reduce the radius below 5 mm. \
         Candidates: Edge2, Edge4."
    );
}

#[test]
fn display_without_optional_fields() {
    assert_eq!(
        Error::new(ErrorCode::Internal, "unexpected state").to_string(),
        "internal: unexpected state"
    );
    assert_eq!(
        Error::new(ErrorCode::InvalidInput, "radius must be positive")
            .with_feature("Fillet1")
            .to_string(),
        "invalid_input: radius must be positive (feature Fillet1)"
    );
    assert_eq!(
        Error::new(ErrorCode::WorkerTimeout, "no reply within 30 s")
            .with_element("Face2")
            .with_suggestion("simplify the model")
            .to_string(),
        "worker_timeout: no reply within 30 s (element Face2). Suggestion: simplify the model."
    );
}

#[test]
fn elements_add_up() {
    let e = Error::new(ErrorCode::InvalidInput, "edges refused")
        .with_element("Edge1")
        .with_element("Edge2");
    assert_eq!(e.elements(), ["Edge1", "Edge2"]);
    assert_eq!(
        e.with_elements(["Edge7", "Edge9"]).elements(),
        ["Edge1", "Edge2", "Edge7", "Edge9"]
    );
    let e = Error::new(ErrorCode::InvalidInput, "dx must be positive").with_parameter("dx");
    assert_eq!(
        e.to_string(),
        "invalid_input: dx must be positive (parameter dx)"
    );
}

#[test]
fn display_doesnt_double_a_period() {
    assert_eq!(
        Error::new(ErrorCode::KernelFailed, "fillet failed")
            .with_cause("Radius too large.")
            .with_suggestion("Reduce the radius.")
            .to_string(),
        "kernel_failed: fillet failed: Radius too large. Suggestion: Reduce the radius."
    );
}

#[test]
fn accessors_return_what_was_set() {
    let e = full();
    assert_eq!(e.code(), ErrorCode::KernelFailed);
    assert_eq!(e.message(), "fillet failed");
    assert_eq!(e.feature(), Some("Fillet1"));
    assert_eq!(e.elements(), ["Edge3", "Edge5"]);
    assert_eq!(e.parameter(), Some("radius"));
    assert_eq!(e.cause(), Some("StdFail_NotDone: BRepFilletAPI_MakeFillet"));
    assert_eq!(e.suggestion(), Some("reduce the radius below 5 mm"));
    assert_eq!(e.candidates(), ["Edge2", "Edge4"]);
}
