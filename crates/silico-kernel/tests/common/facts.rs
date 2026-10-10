//! An operation's history as facts, to compare with what FreeCAD's element names state.

use silico_kernel::{ElementKind, History};

/// One fact of a history: an input element (`Face3`) is kept as, modified into, or
/// generates a result element, at a position in OCCT's list (0 when kept).
pub type Fact = (String, String, String, u32);

/// The history of an operation's first (and only) input, as facts, sorted.
pub fn facts(history: &History) -> Vec<Fact> {
    let mut out = Vec::new();
    let input = &history.inputs[0];
    let kinds = [
        (ElementKind::Face, &input.faces),
        (ElementKind::Edge, &input.edges),
        (ElementKind::Vertex, &input.vertices),
    ];
    for (kind, fates) in kinds {
        for (i, fate) in fates.iter().enumerate() {
            let from = format!("{kind:?}{}", i + 1);
            if let Some(kept) = fate.kept {
                out.push(("kept".into(), from.clone(), format!("{kind:?}{kept}"), 0));
            }
            for (relation, list) in [("modified", &fate.modified), ("generated", &fate.generated)] {
                for r in list {
                    let to = format!("{:?}{}", r.element.kind, r.element.index);
                    out.push((relation.into(), from.clone(), to, r.position));
                }
            }
        }
    }
    out.sort();
    out
}
