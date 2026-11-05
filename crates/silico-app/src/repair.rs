//! A broken reference's repair (the prototype's flow 6b, DESIGN.md's states table; its
//! `errCard` candidates and `useCand`): a fillet's or chamfer's edge whose name its base no
//! longer finds (`reference_broken`, at `edges.N`), with the names the core offers in its
//! place, closest first. Use writes a candidate at the broken name's place, once it's read
//! in the feature's base as its dialog reads a pick there (the core's `resolve_in`,
//! [`Selection::read_on`]), and refuses one that can't be stored there, saying why as the
//! dialogs do. It's one `set_field`, one step for undo; the status bar says what it did, as
//! the prototype's toast does.
//!
//! [`Selection::read_on`]: crate::selection::Selection::read_on

use silico_core::Error;
use silico_core::command::Problems;
use silico_core::document::json::{self, Node};
use silico_core::document::{Document, FeatureId, Operation};

use crate::selection::OnBase;
use crate::tab::{Edit, Said};

/// What the status bar says once a candidate is used (`useCand`'s toast), of the feature
/// `name`: a fillet "rounds" its edges, a chamfer "bevels" them.
pub fn used_words(name: &str, chamfer: bool) -> String {
    let verb = if chamfer { "bevels" } else { "rounds" };
    format!("{name} now {verb} the edge you picked. Undo puts the broken reference back.")
}

/// What's said when the card is of an older document than the session's.
pub const STALE: &str =
    "The model has changed since this card was read: try again once it has rebuilt";

/// A `reference_broken` error on a fillet's or chamfer's edge: the feature, the edge's place
/// in its `edges`, the name as the error gives it (cut short with `…` where it's long), and
/// the names offered in its place, closest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    pub feature: FeatureId,
    pub place: usize,
    pub name: String,
    pub candidates: Vec<String>,
}

impl Broken {
    /// The broken edge `said` is of, if it's a `reference_broken` error at `edges.N`.
    pub fn of(said: &Said) -> Option<Broken> {
        if said.code() != Some("reference_broken") || said.warning {
            return None;
        }
        let feature = FeatureId::new(said.feature()?)?;
        let place = said
            .parameter()?
            .strip_prefix("edges.")?
            .parse::<usize>()
            .ok()?;
        let name = said.elements.first()?.clone();
        Some(Broken {
            feature,
            place,
            name,
            candidates: said.candidates().to_vec(),
        })
    }

    /// Whether `stored` (a name `edges` holds) is the name the error gives: it, or it cut
    /// short (the core gives a long name's first 200 characters and `…`).
    pub fn is(&self, stored: &str) -> bool {
        stored == self.name
            || self
                .name
                .strip_suffix('…')
                .is_some_and(|kept| stored.len() > kept.len() && stored.starts_with(kept))
    }

    /// The field Use sets: the broken name's place in `edges`.
    pub fn field(&self) -> String {
        format!("edges.{}", self.place)
    }
}

/// A fillet or a chamfer, as a document holds it: its base, its edges' names, and whether
/// it's a chamfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dressing {
    pub base: FeatureId,
    pub edges: Vec<String>,
    pub chamfer: bool,
}

impl Dressing {
    /// The feature `id` of `document`, if it's a fillet or a chamfer.
    pub fn of(document: &Document, id: &FeatureId) -> Option<Dressing> {
        let feature = document.features.iter().find(|feature| &feature.id == id)?;
        let (base, chamfer) = match &feature.operation {
            Operation::Fillet { base, .. } => (base, false),
            Operation::Chamfer { base, .. } => (base, true),
            _ => return None,
        };
        Some(Dressing {
            base: base.clone(),
            edges: feature
                .operation
                .edges()
                .iter()
                .map(|edge| edge.as_str().to_owned())
                .collect(),
            chamfer,
        })
    }

    /// Whether `broken`'s place in its edges holds the broken name still.
    fn holds(&self, broken: &Broken) -> bool {
        self.edges
            .get(broken.place)
            .is_some_and(|stored| broken.is(stored))
    }
}

/// `candidate` used in `broken`'s place in `dressing` (the feature `name`, as the model
/// the card was read from holds it), `read` being what reading it in the feature's base
/// gave: the edit that writes the name a store writes for it there at the broken name's
/// place, and what the status bar says of it; or why not, as the dialogs say a refusal.
pub fn used(
    broken: &Broken,
    candidate: &str,
    dressing: &Dressing,
    read: OnBase,
    name: &str,
) -> Result<(Edit, String), String> {
    if !dressing.holds(broken) {
        return Err(STALE.to_owned());
    }
    let stored = match read {
        OnBase::Found(Ok(stored)) => stored,
        OnBase::Found(Err(refusal)) => return Err(words(&refusal)),
        OnBase::Missing => {
            return Err(format!(
                "{candidate} isn't on {}, which {name} is built on: it can't take that edge",
                dressing.base
            ));
        }
    };
    let value = string(&stored.to_string())?;
    Ok((
        Edit {
            feature: broken.feature.clone(),
            field: broken.field(),
            value,
        },
        used_words(name, dressing.chamfer),
    ))
}

/// `text` as the document reads a string.
fn string(text: &str) -> Result<Node, String> {
    json::read(crate::dialog::quoted(text).as_bytes())
        .map_err(|_| format!("{text} doesn't read as a name"))
}

/// What `error` says, its message and its suggestion, as the tab says a refusal.
fn words(error: &Error) -> String {
    crate::tab::said(&Problems::from(error.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_core::ErrorCode;
    use silico_core::naming::Name;
    use std::sync::Arc;

    fn id(text: &str) -> FeatureId {
        FeatureId::new(text).unwrap()
    }

    fn broken(place: usize, name: &str) -> Broken {
        Broken {
            feature: id("f0000001"),
            place,
            name: name.to_owned(),
            candidates: vec!["k7f2qx3m.box.Edge4".to_owned()],
        }
    }

    fn fillet(edges: &[&str]) -> Dressing {
        Dressing {
            base: id("m3tq8v0z"),
            edges: edges.iter().map(|edge| (*edge).to_owned()).collect(),
            chamfer: false,
        }
    }

    /// The box `k7f2qx3m`'s edge `n`, by its own name.
    fn edge(n: u32) -> Arc<Name> {
        Arc::new(Name::Element {
            feature: id("k7f2qx3m"),
            made: silico_core::naming::TypeWord::Primitive(silico_core::naming::PrimitiveKind::Box),
            element: silico_protocol::ElementRef {
                kind: silico_protocol::ElementKind::Edge,
                index: n,
            },
        })
    }

    #[test]
    fn a_broken_reference_is_read_from_its_error_at_its_place_in_edges() {
        let error = Error::new(ErrorCode::ReferenceBroken, "edges.1 is gone")
            .with_feature("f0000001")
            .with_parameter("edges.1")
            .with_element("k7f2qx3m.box.Edge5")
            .with_candidates(vec![
                "k7f2qx3m.box.Edge4".to_owned(),
                "k7f2qx3m.box.Edge6".to_owned(),
            ]);
        let found = Broken::of(&Said::of(&error)).unwrap();
        assert_eq!(found.feature, id("f0000001"));
        assert_eq!(found.place, 1);
        assert_eq!(found.name, "k7f2qx3m.box.Edge5");
        assert_eq!(
            found.candidates,
            ["k7f2qx3m.box.Edge4", "k7f2qx3m.box.Edge6"]
        );
        assert_eq!(found.field(), "edges.1");
        // Another code, or a selection's refusal (not at `edges.N`), isn't one.
        let selection = Error::new(ErrorCode::ReferenceBroken, "unstable")
            .with_feature("f0000001")
            .with_parameter("selection.0")
            .with_element("k7f2qx3m.box.Edge5");
        assert_eq!(Broken::of(&Said::of(&selection)), None);
        let other = Error::new(ErrorCode::KernelFailed, "no")
            .with_feature("f0000001")
            .with_parameter("edges.0")
            .with_element("k7f2qx3m.box.Edge5");
        assert_eq!(Broken::of(&Said::of(&other)), None);
    }

    #[test]
    fn a_long_name_cut_short_is_the_name_it_was_cut_from() {
        let long = format!("k7f2qx3m.box.Edge5{}", "x".repeat(300));
        let cut: String = long.chars().take(200).collect();
        let broken = broken(0, &format!("{cut}…"));
        assert!(broken.is(&long));
        assert!(!broken.is(&cut));
        assert!(!broken.is("k7f2qx3m.box.Edge4"));
    }

    #[test]
    fn use_writes_the_name_stored_on_the_base_at_the_broken_names_place() {
        let dressing = fillet(&["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge5"]);
        let read = OnBase::Found(Ok(edge(4)));
        let (edit, told) = used(
            &broken(1, "k7f2qx3m.box.Edge5"),
            "k7f2qx3m.box.Edge4",
            &dressing,
            read,
            "Inside corner",
        )
        .unwrap();
        assert_eq!(edit.feature, id("f0000001"));
        assert_eq!(edit.field, "edges.1");
        assert_eq!(edit.value, json::read(br#""k7f2qx3m.box.Edge4""#).unwrap());
        assert_eq!(
            told,
            "Inside corner now rounds the edge you picked. Undo puts the broken reference back."
        );
    }

    #[test]
    fn use_refuses_an_unstable_candidate_with_its_reason_and_one_off_the_base() {
        let dressing = fillet(&["k7f2qx3m.box.Edge5"]);
        let unstable = Error::new(
            ErrorCode::ReferenceBroken,
            "edges.0, \"m3tq8v0z.cut.Edge5\", names an edge, whose name is unstable",
        )
        .with_suggestion("name the edge as its feature now names it");
        let refused = used(
            &broken(0, "k7f2qx3m.box.Edge5"),
            "m3tq8v0z.cut.Edge5",
            &dressing,
            OnBase::Found(Err(unstable)),
            "Inside corner",
        )
        .unwrap_err();
        assert_eq!(
            refused,
            "edges.0, \"m3tq8v0z.cut.Edge5\", names an edge, whose name is unstable (name the \
             edge as its feature now names it)"
        );
        let missing = used(
            &broken(0, "k7f2qx3m.box.Edge5"),
            "a91cw0de.cylinder.Edge1",
            &dressing,
            OnBase::Missing,
            "Inside corner",
        )
        .unwrap_err();
        assert_eq!(
            missing,
            "a91cw0de.cylinder.Edge1 isn't on m3tq8v0z, which Inside corner is built on: it \
             can't take that edge"
        );
    }

    #[test]
    fn a_card_older_than_the_document_changes_nothing() {
        // The place holds another name now: Use doesn't write over it.
        let dressing = fillet(&["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge4"]);
        let stale = broken(1, "k7f2qx3m.box.Edge5");
        let read = OnBase::Found(Ok(edge(6)));
        assert_eq!(
            used(&stale, "k7f2qx3m.box.Edge6", &dressing, read, "F").unwrap_err(),
            STALE
        );
        // Its place past the edges, the same.
        let read = OnBase::Found(Ok(edge(6)));
        assert_eq!(
            used(
                &broken(2, "k7f2qx3m.box.Edge5"),
                "k7f2qx3m.box.Edge6",
                &dressing,
                read,
                "F"
            )
            .unwrap_err(),
            STALE
        );
    }
}
