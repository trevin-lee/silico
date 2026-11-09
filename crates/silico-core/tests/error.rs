//! M0.5: the structured error. Its JSON form is the contract with agents, so it's checked
//! against committed files (tests/contract/), and changing it means changing them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::{Error, ErrorCode, Nearest, Rank, Warning, WarningCode};

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
        .with_ranked(ranked())
}

/// [`full`]'s candidates, each with its rank, every field of each filled.
fn ranked() -> [(&'static str, Rank); 2] {
    [
        (
            "Edge2",
            Rank {
                shared: vec!["Face1".to_owned()],
                unshared: vec!["Face4".to_owned()],
                same_feature: true,
                nearest: Some(Nearest {
                    element: "Edge2".to_owned(),
                    gap: 1,
                }),
            },
        ),
        (
            "Edge4",
            Rank {
                shared: vec!["Face4".to_owned()],
                unshared: vec!["Face1".to_owned()],
                same_feature: false,
                nearest: Some(Nearest {
                    element: "Edge4".to_owned(),
                    gap: 1,
                }),
            },
        ),
    ]
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
fn warning_codes_are_stable() {
    // Decision 17: the warning codes are a list of their own, kept as the error codes are.
    let expected: Vec<String> = contract("warning-codes.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(str::to_owned)
        .collect();
    let actual: Vec<String> = WarningCode::ALL
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();
    assert_eq!(actual, expected);
    for code in WarningCode::ALL {
        assert_eq!(
            serde_json::to_value(code).unwrap(),
            serde_json::Value::String(code.as_str().to_owned())
        );
    }
    // No code is both a warning's and an error's: an agent tells them apart by code alone.
    for warning in WarningCode::ALL {
        assert!(
            ErrorCode::ALL
                .iter()
                .all(|e| e.as_str() != warning.as_str()),
            "{warning}"
        );
    }
}

#[test]
fn a_warning_is_the_error_contracts_shape() {
    let warning = Warning::new(WarningCode::EndsNotJoined, "fillet failed")
        .with_feature("Fillet1")
        .with_elements(["Edge3", "Edge5"])
        .with_parameter("radius")
        .with_cause("StdFail_NotDone: BRepFilletAPI_MakeFillet")
        .with_suggestion("reduce the radius below 5 mm");
    // error-full.json's error, but its candidates: its shape is pinned against the file
    // above, so the warning's is that, with its own code.
    let error = Error::new(ErrorCode::KernelFailed, "fillet failed")
        .with_feature("Fillet1")
        .with_elements(["Edge3", "Edge5"])
        .with_parameter("radius")
        .with_cause("StdFail_NotDone: BRepFilletAPI_MakeFillet")
        .with_suggestion("reduce the radius below 5 mm");
    assert_eq!(
        serde_json::to_string_pretty(&warning).unwrap(),
        serde_json::to_string_pretty(&error).unwrap().replacen(
            "\"kernel_failed\"",
            "\"ends_not_joined\"",
            1
        )
    );
    assert_eq!(
        warning.to_string(),
        "ends_not_joined: fillet failed (feature Fillet1, elements Edge3, Edge5, parameter \
         radius): StdFail_NotDone: BRepFilletAPI_MakeFillet. Suggestion: reduce the radius \
         below 5 mm."
    );
    let warning = warning.with_elements_total(7);
    assert_eq!(warning.elements_total(), 7);
    assert_eq!(warning.elements(), ["Edge3", "Edge5"]);
    assert_eq!(warning.with_elements_total(1).elements_total(), 2);
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
fn the_total_counts_the_elements_named_and_any_beyond_them() {
    // Named, the total is how many are named, as elements are added.
    let e = Error::new(ErrorCode::KernelFailed, "mesh failed").with_element("Face2");
    assert_eq!(e.elements_total(), 1);
    let e = e.with_elements(["Face4", "Face6"]);
    assert_eq!(e.elements_total(), 3);
    // Too many to name: the total says how many there are.
    let e = e.with_elements_total(250);
    assert_eq!(e.elements_total(), 250);
    assert_eq!(e.elements(), ["Face2", "Face4", "Face6"]);
    assert_eq!(
        e.to_string(),
        "kernel_failed: mesh failed (elements Face2, Face4, Face6, and 247 more)"
    );
    assert_eq!(serde_json::to_value(&e).unwrap()["elements_total"], 250);
    // Never fewer than are named, whichever is set first.
    let e = Error::new(ErrorCode::KernelFailed, "mesh failed")
        .with_elements_total(1)
        .with_elements(["Face1", "Face2"]);
    assert_eq!(e.elements_total(), 2);
    let e = e.with_elements_total(0);
    assert_eq!(e.elements_total(), 2);
    assert_eq!(
        e.to_string(),
        "kernel_failed: mesh failed (elements Face1, Face2)"
    );
    // One named of several is still "elements"; none named, only counted.
    assert_eq!(
        Error::new(ErrorCode::KernelFailed, "mesh failed")
            .with_element("Face1")
            .with_elements_total(2)
            .to_string(),
        "kernel_failed: mesh failed (elements Face1, and 1 more)"
    );
    assert_eq!(
        Error::new(ErrorCode::KernelFailed, "mesh failed")
            .with_feature("Pad1")
            .with_elements_total(4)
            .with_parameter("deflection")
            .to_string(),
        "kernel_failed: mesh failed (feature Pad1, 4 elements, parameter deflection)"
    );
    assert_eq!(
        Error::new(ErrorCode::KernelFailed, "mesh failed")
            .with_elements_total(1)
            .to_string(),
        "kernel_failed: mesh failed (1 element)"
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
    assert_eq!(e.elements_total(), 2);
    assert_eq!(e.parameter(), Some("radius"));
    assert_eq!(e.cause(), Some("StdFail_NotDone: BRepFilletAPI_MakeFillet"));
    assert_eq!(e.suggestion(), Some("reduce the radius below 5 mm"));
    assert_eq!(e.candidates(), ["Edge2", "Edge4"]);
    let ranks: Vec<Rank> = ranked().into_iter().map(|(_, rank)| rank).collect();
    assert_eq!(e.ranking(), ranks);
}

#[test]
fn candidates_and_their_ranking_are_set_together_and_unranked_candidates_clear_it() {
    // `with_ranked` sets both, one rank a candidate in their order; `with_candidates`, a
    // list by another rule, leaves no ranking of the candidates it replaced.
    let ranked_error = full();
    assert_eq!(ranked_error.candidates(), ["Edge2", "Edge4"]);
    assert_eq!(ranked_error.ranking().len(), 2);
    assert_eq!(
        ranked_error.ranking()[0].nearest.as_ref().unwrap().element,
        "Edge2"
    );
    assert_eq!(
        ranked_error.ranking()[1].nearest.as_ref().unwrap().element,
        "Edge4"
    );
    let unranked = full().with_candidates(["+x", "-x"]);
    assert_eq!(unranked.candidates(), ["+x", "-x"]);
    assert!(unranked.ranking().is_empty());
    // Ranked with none: neither.
    let none = full().with_ranked(Vec::<(String, Rank)>::new());
    assert!(none.candidates().is_empty() && none.ranking().is_empty());
    // An error never ranked, and a warning, write `ranking` as `[]`.
    let minimal = serde_json::to_value(Error::new(ErrorCode::Internal, "m")).unwrap();
    assert_eq!(minimal["ranking"], serde_json::json!([]));
    let warning = serde_json::to_value(Warning::new(WarningCode::EndsNotJoined, "apart")).unwrap();
    assert_eq!(warning["ranking"], serde_json::json!([]));
}

#[test]
fn errors_are_equal_by_every_field_they_say_and_no_fewer() {
    // Found by the mutation run: equality that held of any two errors passed every test,
    // since every comparison of errors is an assert_eq. Each error here differs from
    // full() in one field.
    assert_eq!(full(), full());
    let full_as = |code, message| {
        Error::new(code, message)
            .with_feature("Fillet1")
            .with_element("Edge3")
            .with_element("Edge5")
            .with_parameter("radius")
            .with_cause("StdFail_NotDone: BRepFilletAPI_MakeFillet")
            .with_suggestion("reduce the radius below 5 mm")
            .with_ranked(ranked())
    };
    assert_eq!(full_as(ErrorCode::KernelFailed, "fillet failed"), full());
    let apart = [
        full_as(ErrorCode::InvalidInput, "fillet failed"),
        full_as(ErrorCode::KernelFailed, "chamfer failed"),
        full().with_feature("Fillet2"),
        full().with_element("Edge7"),
        full().with_elements_total(9),
        full().with_parameter("angle"),
        full().with_cause("another cause"),
        full().with_suggestion("another suggestion"),
        full().with_candidates(["Edge2"]),
        // The same candidates, unranked; or ranked otherwise.
        full().with_candidates(["Edge2", "Edge4"]),
        full().with_ranked(ranked().map(|(candidate, rank)| {
            (
                candidate,
                Rank {
                    same_feature: !rank.same_feature,
                    ..rank
                },
            )
        })),
    ];
    for other in &apart {
        assert_ne!(&full(), other);
        assert_ne!(other, &full());
    }
    // A field unset is not a field set.
    assert_ne!(
        Error::new(ErrorCode::KernelFailed, "fillet failed"),
        Error::new(ErrorCode::KernelFailed, "fillet failed").with_feature("Fillet1")
    );
}

#[test]
fn a_conflict_is_no_kernel_fault() {
    // Decision 16: an edit against another revision, or a commit over a file changed
    // outside the session, is fixed by rolling back or reading again; the kernel's no part.
    assert_eq!(ErrorCode::Conflict.as_str(), "conflict");
    assert!(!ErrorCode::Conflict.is_kernel_fault());
}

/// The codes a `.txt` contract lists, in order.
fn listed(name: &str) -> Vec<serde_json::Value> {
    contract(name)
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|code| serde_json::Value::String(code.to_owned()))
        .collect()
}

/// Every `code` property's `enum` of every definition named `def` in `schema`, at any
/// depth, by the path it's at.
fn code_enums(
    schema: &serde_json::Value,
    def: &str,
    at: &str,
    found: &mut Vec<(String, serde_json::Value)>,
) {
    match schema {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let here = format!("{at}/{key}");
                if key == "$defs"
                    && let Some(defined) = value.get(def)
                {
                    found.push((
                        format!("{here}/{def}"),
                        defined["properties"]["code"]["enum"].clone(),
                    ));
                }
                code_enums(value, def, &here, found);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                code_enums(item, def, &format!("{at}/{i}"), found);
            }
        }
        _ => {}
    }
}

#[test]
fn every_schemas_codes_are_the_contracts_lists_in_their_order() {
    // An agent reads the codes from the schema it's given (`tools/list`, `schema`), never
    // from the repository: each `Error`'s `code` is an enum of error-codes.txt's codes, and
    // each `Warning`'s of warning-codes.txt's, in each file's order, in every schema the
    // core generates that holds one (each command's, and the envelope's).
    use silico_core::command::wire::{COMMANDS, snapshot_value};
    let errors = serde_json::Value::Array(listed("error-codes.txt"));
    let warnings = serde_json::Value::Array(listed("warning-codes.txt"));
    let mut holding = Vec::new();
    let names = COMMANDS.iter().map(|(name, _)| *name).chain(["envelope"]);
    for name in names {
        let schema = snapshot_value(name).unwrap();
        for (def, expected) in [("Error", &errors), ("Warning", &warnings)] {
            let mut found = Vec::new();
            code_enums(&schema, def, name, &mut found);
            for (at, codes) in found {
                assert_eq!(&codes, expected, "{at}");
                holding.push(at);
            }
        }
    }
    // The envelope's Error, and check's Error and Warning, at least.
    for at in [
        "envelope/$defs/Error",
        "check/result/$defs/Error",
        "check/result/$defs/Warning",
    ] {
        assert!(holding.iter().any(|held| held == at), "{at}: {holding:?}");
    }
}
