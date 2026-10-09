//! A history from a worker is checked before it's used: it's for the right number of
//! inputs, and names only elements the result has.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_protocol::{
    ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory, Reported,
};

const RESULT: ElementCounts = ElementCounts {
    faces: 2,
    edges: 3,
    vertices: 4,
    solids: 1,
};

fn reported(kind: ElementKind, index: u32) -> Reported {
    Reported {
        element: ElementRef { kind, index },
        position: 1,
    }
}

fn fate(kept: Option<u32>, modified: Vec<Reported>, generated: Vec<Reported>) -> ElementFate {
    ElementFate {
        kept,
        modified,
        generated,
    }
}

/// A one-input history with `fate` as the input's only face.
fn with_face(fate: ElementFate) -> History {
    History {
        inputs: vec![InputHistory {
            faces: vec![fate],
            edges: vec![],
            vertices: vec![],
        }],
        result: RESULT,
    }
}

#[test]
fn a_history_names_only_what_the_result_has() {
    let good = fate(
        Some(2),
        vec![reported(ElementKind::Face, 1)],
        // Sorted by element: vertices, then edges, faces, solids.
        vec![
            reported(ElementKind::Vertex, 4),
            reported(ElementKind::Edge, 3),
            reported(ElementKind::Solid, 1),
        ],
    );
    assert_eq!(with_face(good).validate(1), Ok(()));
    for (bad, says) in [
        (fate(Some(3), vec![], vec![]), "kept as Face3"),
        (fate(Some(0), vec![], vec![]), "kept as Face0"),
        (
            fate(None, vec![reported(ElementKind::Face, 3)], vec![]),
            "names Face3",
        ),
        (
            fate(None, vec![], vec![reported(ElementKind::Edge, 4)]),
            "names Edge4",
        ),
        (
            fate(None, vec![], vec![reported(ElementKind::Vertex, 0)]),
            "names Vertex0",
        ),
        (
            fate(None, vec![], vec![reported(ElementKind::Solid, 2)]),
            "names Solid2",
        ),
    ] {
        let err = with_face(bad).validate(1).unwrap_err();
        assert!(err.contains(says), "{err}");
        assert!(err.starts_with("input 1 Face1"), "{err}");
    }
}

#[test]
fn a_history_is_checked_for_every_kind_and_every_input() {
    let bad = fate(Some(99), vec![], vec![]);
    let input = |faces: Vec<ElementFate>, edges: Vec<ElementFate>, vertices: Vec<ElementFate>| {
        InputHistory {
            faces,
            edges,
            vertices,
        }
    };
    let history = |inputs| History {
        inputs,
        result: RESULT,
    };
    let edge = history(vec![input(vec![], vec![bad.clone()], vec![])]);
    assert!(edge.validate(1).unwrap_err().starts_with("input 1 Edge1"));
    let vertex = history(vec![input(vec![], vec![], vec![bad.clone()])]);
    assert!(
        vertex
            .validate(1)
            .unwrap_err()
            .starts_with("input 1 Vertex1")
    );
    let second = history(vec![
        InputHistory::default(),
        input(vec![bad], vec![], vec![]),
    ]);
    assert!(second.validate(2).unwrap_err().starts_with("input 2 Face1"));
}

#[test]
fn a_history_is_for_as_many_inputs_as_the_operation_had() {
    let none = History {
        inputs: vec![],
        result: RESULT,
    };
    assert!(none.validate(1).unwrap_err().contains("for 0 input shapes"));
    let one = with_face(ElementFate::default());
    assert!(one.validate(2).unwrap_err().contains("operation had 2"));
    assert_eq!(one.validate(1), Ok(()));
}

#[test]
fn a_fate_is_read_without_indexing_past_the_end() {
    let h = with_face(fate(Some(2), vec![], vec![]));
    let face = |index| ElementRef {
        kind: ElementKind::Face,
        index,
    };
    assert_eq!(h.fate(0, face(1)).unwrap().kept, Some(2));
    assert!(h.fate(0, face(0)).is_none());
    assert!(h.fate(0, face(2)).is_none());
    assert!(h.fate(1, face(1)).is_none());
    let edge = ElementRef {
        kind: ElementKind::Edge,
        index: 1,
    };
    assert!(h.fate(0, edge).is_none());
    let solid = ElementRef {
        kind: ElementKind::Solid,
        index: 1,
    };
    assert!(h.fate(0, solid).is_none());
}

#[test]
fn a_deleted_element_is_neither_kept_nor_modified() {
    let r = reported(ElementKind::Face, 1);
    assert!(fate(None, vec![], vec![]).is_deleted());
    assert!(fate(None, vec![], vec![r]).is_deleted());
    assert!(!fate(Some(1), vec![], vec![]).is_deleted());
    assert!(!fate(None, vec![r], vec![]).is_deleted());
}

#[test]
fn a_fate_is_read_from_its_own_kinds_list() {
    let mark = |n| fate(Some(n), vec![], vec![]);
    let h = History {
        inputs: vec![InputHistory {
            faces: vec![mark(1)],
            edges: vec![mark(2)],
            vertices: vec![mark(3)],
        }],
        result: RESULT,
    };
    let first = |kind| ElementRef { kind, index: 1 };
    assert_eq!(h.fate(0, first(ElementKind::Face)).unwrap().kept, Some(1));
    assert_eq!(h.fate(0, first(ElementKind::Edge)).unwrap().kept, Some(2));
    assert_eq!(h.fate(0, first(ElementKind::Vertex)).unwrap().kept, Some(3));
}

#[test]
fn a_history_keeps_the_promises_its_types_make() {
    // From another process, a history is checked for what Silico's own histories always
    // are: modified into the same kind, positions from 1, lists sorted without repeats.
    let at = |kind, index, position| Reported {
        element: ElementRef { kind, index },
        position,
    };
    let cases = [
        (
            fate(None, vec![at(ElementKind::Edge, 1, 1)], vec![]),
            "another kind",
        ),
        (
            fate(None, vec![at(ElementKind::Face, 1, 0)], vec![]),
            "position 0",
        ),
        (
            fate(None, vec![], vec![at(ElementKind::Edge, 1, 0)]),
            "position 0",
        ),
        (
            fate(
                None,
                vec![],
                vec![at(ElementKind::Edge, 2, 1), at(ElementKind::Edge, 1, 2)],
            ),
            "isn't sorted",
        ),
        (
            fate(
                None,
                vec![at(ElementKind::Face, 1, 1), at(ElementKind::Face, 1, 2)],
                vec![],
            ),
            "repeats",
        ),
    ];
    for (bad, says) in cases {
        let err = with_face(bad).validate(1).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
    let good = fate(
        None,
        vec![at(ElementKind::Face, 1, 2)],
        vec![at(ElementKind::Edge, 1, 3), at(ElementKind::Edge, 2, 1)],
    );
    assert_eq!(with_face(good).validate(1), Ok(()));
}
