//! A broken reference's repair (the prototype's flow 6b, DESIGN.md's states table; its
//! `errCard` candidates, `useCand`, and `removeRef`): a fillet's or chamfer's edge whose
//! name its base no longer finds (`reference_broken`, at `edges.N`), with the names the core
//! offers in its place, closest first. Use writes a candidate at the broken name's place,
//! once it's read in the feature's base as its dialog reads a pick there (the core's
//! `resolve_in`, [`Selection::read_on`]), and refuses one that can't be stored there,
//! saying why as the dialogs do. Remove the edge drops the broken name from `edges`, but
//! never a feature's last edge: the gate refuses a fillet or a chamfer that names none, and
//! SolidWorks' Fillet PropertyManager, its missing references cleared, is completed by
//! selecting edges again (*Repairing Missing References for Fillets*), never left with
//! none. Each is one `set_field`, one step for undo; the status bar says what it did, as
//! the prototype's toast does.
//!
//! [`Selection::read_on`]: crate::selection::Selection::read_on

use silico_core::Error;
use silico_core::command::{Problems, Revision};
use silico_core::document::json::{self, Node};
use silico_core::document::{Document, FeatureId, Operation};

use crate::selection::OnBase;
use crate::tab::{Edit, Said};

/// Where a repair was made, which says how it's taken back: on the document, by Undo; or
/// in the feature's open dialog, by its Cancel (Undo is off while a dialog is open, and
/// after its OK one Undo takes back all of the dialog's edits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Made {
    Document,
    Dialog,
}

impl Made {
    /// How the broken reference is put back.
    fn back(self) -> &'static str {
        match self {
            Made::Document => "Undo puts the broken reference back.",
            Made::Dialog => {
                "Cancel puts the broken reference back, as Undo does once the dialog is OK'd."
            }
        }
    }
}

/// What the status bar says once a candidate is used (`useCand`'s toast), of the feature
/// `name`: a fillet "rounds" its edges, a chamfer "bevels" them.
pub fn used_words(name: &str, chamfer: bool, made: Made) -> String {
    let verb = if chamfer { "bevels" } else { "rounds" };
    format!("{name} now {verb} the edge you picked. {}", made.back())
}

/// What the status bar says once the broken name is removed.
pub fn removed_words(name: &str, made: Made) -> String {
    format!("{name} no longer names the broken edge. {}", made.back())
}

/// The words of the step a candidate's use makes of the feature `name`, as Undo and Redo
/// name it: the prototype's `useCand` label (`'Inside corner: use the inside corner edge'`),
/// the edge the one the person picked, as the status bar says it.
pub fn used_step(name: &str) -> String {
    format!("{name}: use the edge you picked")
}

/// The words of the step the broken name's removal makes of the feature `name`: the
/// prototype's `removeRef` label (`'Inside corner: remove its edge'`).
pub fn removed_step(name: &str) -> String {
    format!("{name}: remove its edge")
}

/// Why Remove the edge is off for the feature `name`'s last edge.
pub fn last_edge_words(name: &str, chamfer: bool) -> String {
    let what = if chamfer { "chamfer" } else { "fillet" };
    format!(
        "{name} names no other edge, and a {what} names at least one: use a candidate in its \
         place, or suppress or delete {name}"
    )
}

/// What's said when the card is of an older document than the session's, or its broken
/// name is found now.
pub const STALE: &str =
    "The model has changed since this card was read: try again once it has rebuilt";

/// A `reference_broken` error on a fillet's or chamfer's edge: the feature, the edge's place
/// in its `edges`, the name as the error gives it (cut short with `…` where it's long), and
/// the names offered in its place, closest first; and the session's revision the card was
/// read at (its `check`'s timeline's), which a repair is made at or not at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    pub feature: FeatureId,
    pub place: usize,
    pub name: String,
    pub candidates: Vec<String>,
    pub read: Revision,
}

impl Broken {
    /// The broken edge `said` is of, if it's a `reference_broken` error at `edges.N`, read
    /// at the revision `read`.
    pub fn of(said: &Said, read: Revision) -> Option<Broken> {
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
            read,
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

    /// Whether `broken` is broken still, `read` reading a name in the feature's base as
    /// its dialog reads a pick there ([`Selection::read_on`]): its place in the edges holds
    /// the broken name, and that name, as the edges hold it in full, finds no edge a store
    /// could take there now. Or [`STALE`]: whatever was read when, the card is of another
    /// document than this, and a repair would write over a name that isn't broken.
    ///
    /// [`Selection::read_on`]: crate::selection::Selection::read_on
    fn still_broken(
        &self,
        broken: &Broken,
        read: &dyn Fn(&str, &FeatureId) -> OnBase,
    ) -> Result<(), String> {
        let Some(stored) = self
            .edges
            .get(broken.place)
            .filter(|stored| broken.is(stored))
        else {
            return Err(STALE.to_owned());
        };
        match read(stored, &self.base) {
            OnBase::Found(Ok(_)) => Err(STALE.to_owned()),
            OnBase::Found(Err(_)) | OnBase::Missing => Ok(()),
        }
    }

    /// Whether Remove the edge can drop `broken`, or why not: the feature `name`'s last edge.
    pub fn removable(&self, name: &str) -> Result<(), String> {
        if self.edges.len() <= 1 {
            Err(last_edge_words(name, self.chamfer))
        } else {
            Ok(())
        }
    }
}

/// `candidate` used in `broken`'s place in `dressing` (the feature `name`, as the current
/// model's document holds it), `read` reading a name in the feature's base: the edit that
/// writes the name a store writes for the candidate there at the broken name's place, and
/// what the status bar says of it; or why not, as the dialogs say a refusal (the name not
/// broken now among them).
pub fn used(
    broken: &Broken,
    candidate: &str,
    dressing: &Dressing,
    read: &dyn Fn(&str, &FeatureId) -> OnBase,
    name: &str,
) -> Result<(Edit, String), String> {
    dressing.still_broken(broken, read)?;
    let stored = match read(candidate, &dressing.base) {
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
        used_words(name, dressing.chamfer, Made::Document),
    ))
}

/// `broken` removed from `dressing`'s edges (the feature `name`), `read` reading a name in
/// its base: the edit that sets `edges` without it, and what the status bar says of it; or
/// why not (its last edge, or the card of another document, its name not broken now).
pub fn removed(
    broken: &Broken,
    dressing: &Dressing,
    read: &dyn Fn(&str, &FeatureId) -> OnBase,
    name: &str,
) -> Result<(Edit, String), String> {
    dressing.still_broken(broken, read)?;
    dressing.removable(name)?;
    let kept: Vec<String> = dressing
        .edges
        .iter()
        .enumerate()
        .filter(|(at, _)| *at != broken.place)
        .map(|(_, edge)| crate::dialog::quoted(edge))
        .collect();
    let value = json::read(format!("[{}]", kept.join(", ")).as_bytes())
        .map_err(|_| "the edges left don't read as names".to_owned())?;
    Ok((
        Edit {
            feature: broken.feature.clone(),
            field: "edges".to_owned(),
            value,
        },
        removed_words(name, Made::Document),
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

    /// The revision a card in these tests was read at.
    const READ: Revision = Revision::OPENED;

    fn broken(place: usize, name: &str) -> Broken {
        Broken {
            feature: id("f0000001"),
            place,
            name: name.to_owned(),
            candidates: vec!["k7f2qx3m.box.Edge4".to_owned()],
            read: READ,
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

    /// The names a [`reading`] was asked, in order.
    type Asked = Arc<std::sync::Mutex<Vec<String>>>;

    /// A read of names in the base `m3tq8v0z`: each of `answers` by its name, the rest
    /// missing (broken), as the base no longer holds them. Every name read is recorded.
    fn reading(
        answers: Vec<(&'static str, OnBase)>,
    ) -> (impl Fn(&str, &FeatureId) -> OnBase, Asked) {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = Arc::clone(&asked);
        let read = move |name: &str, base: &FeatureId| {
            assert_eq!(base, &id("m3tq8v0z"), "read in the feature's base");
            record.lock().unwrap().push(name.to_owned());
            answers
                .iter()
                .find(|(known, _)| *known == name)
                .map_or(OnBase::Missing, |(_, answer)| answer.clone())
        };
        (read, asked)
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
        let at = Revision::from_number(7).unwrap();
        let found = Broken::of(&Said::of(&error), at).unwrap();
        assert_eq!(found.feature, id("f0000001"));
        assert_eq!(found.place, 1);
        assert_eq!(found.name, "k7f2qx3m.box.Edge5");
        assert_eq!(
            found.candidates,
            ["k7f2qx3m.box.Edge4", "k7f2qx3m.box.Edge6"]
        );
        assert_eq!(found.read, at);
        assert_eq!(found.field(), "edges.1");
        // Another code, or a selection's refusal (not at `edges.N`), isn't one.
        let selection = Error::new(ErrorCode::ReferenceBroken, "unstable")
            .with_feature("f0000001")
            .with_parameter("selection.0")
            .with_element("k7f2qx3m.box.Edge5");
        assert_eq!(Broken::of(&Said::of(&selection), at), None);
        let other = Error::new(ErrorCode::KernelFailed, "no")
            .with_feature("f0000001")
            .with_parameter("edges.0")
            .with_element("k7f2qx3m.box.Edge5");
        assert_eq!(Broken::of(&Said::of(&other), at), None);
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
        // The candidate as the cut names the edge it keeps from the block; a store writes
        // the block's own name for it, which is what's written, not the candidate's text.
        let dressing = fillet(&["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge5"]);
        let (read, asked) = reading(vec![("m3tq8v0z.cut.Edge4", OnBase::Found(Ok(edge(4))))]);
        let (edit, told) = used(
            &broken(1, "k7f2qx3m.box.Edge5"),
            "m3tq8v0z.cut.Edge4",
            &dressing,
            &read,
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
        // The broken name read first, as the edges hold it, then the candidate.
        assert_eq!(
            *asked.lock().unwrap(),
            ["k7f2qx3m.box.Edge5", "m3tq8v0z.cut.Edge4"]
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
        let (read, _) = reading(vec![("m3tq8v0z.cut.Edge5", OnBase::Found(Err(unstable)))]);
        let refused = used(
            &broken(0, "k7f2qx3m.box.Edge5"),
            "m3tq8v0z.cut.Edge5",
            &dressing,
            &read,
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
            &read,
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
        // The place holds another name now: neither Use nor Remove writes over it.
        let dressing = fillet(&["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge4"]);
        let stale = broken(1, "k7f2qx3m.box.Edge5");
        let (read, _) = reading(vec![("k7f2qx3m.box.Edge6", OnBase::Found(Ok(edge(6))))]);
        assert_eq!(
            used(&stale, "k7f2qx3m.box.Edge6", &dressing, &read, "F").unwrap_err(),
            STALE
        );
        assert_eq!(removed(&stale, &dressing, &read, "F").unwrap_err(), STALE);
        // Its place past the edges, the same.
        assert_eq!(
            removed(&broken(2, "k7f2qx3m.box.Edge5"), &dressing, &read, "F").unwrap_err(),
            STALE
        );
    }

    #[test]
    fn a_card_whose_broken_name_is_found_now_changes_nothing() {
        // The place holds the broken name still, but the base finds it now (an undo of the
        // edit that broke it, read before the card was): neither Use nor Remove writes over
        // a name that isn't broken. A name the base refuses (unstable there) is broken still.
        let dressing = fillet(&["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge5"]);
        let card = broken(1, "k7f2qx3m.box.Edge5");
        let (mended, _) = reading(vec![
            ("k7f2qx3m.box.Edge5", OnBase::Found(Ok(edge(5)))),
            ("k7f2qx3m.box.Edge4", OnBase::Found(Ok(edge(4)))),
        ]);
        assert_eq!(
            used(&card, "k7f2qx3m.box.Edge4", &dressing, &mended, "F").unwrap_err(),
            STALE
        );
        assert_eq!(removed(&card, &dressing, &mended, "F").unwrap_err(), STALE);
        let unstable = Error::new(ErrorCode::ReferenceBroken, "unstable");
        let (refused, _) = reading(vec![
            ("k7f2qx3m.box.Edge5", OnBase::Found(Err(unstable))),
            ("k7f2qx3m.box.Edge4", OnBase::Found(Ok(edge(4)))),
        ]);
        assert!(used(&card, "k7f2qx3m.box.Edge4", &dressing, &refused, "F").is_ok());
        assert!(removed(&card, &dressing, &refused, "F").is_ok());
        // A long name, cut short in the card, is read as the edges hold it, in full.
        let long = format!("k7f2qx3m.box.Edge5{}", "x".repeat(300));
        let cut: String = long.chars().take(200).collect();
        let long_dressing = Dressing {
            edges: vec!["k7f2qx3m.box.Edge1".to_owned(), long.clone()],
            ..dressing
        };
        let (read, asked) = reading(vec![]);
        let _ = removed(&broken(1, &format!("{cut}…")), &long_dressing, &read, "F");
        assert_eq!(*asked.lock().unwrap(), [long]);
    }

    #[test]
    fn remove_drops_the_broken_name_alone_but_never_the_last_edge() {
        let dressing = fillet(&[
            "k7f2qx3m.box.Edge1",
            "k7f2qx3m.box.Edge5",
            "k7f2qx3m.box.Edge9",
        ]);
        let (read, _) = reading(vec![]);
        let (edit, told) = removed(
            &broken(1, "k7f2qx3m.box.Edge5"),
            &dressing,
            &read,
            "Inside corner",
        )
        .unwrap();
        assert_eq!(edit.field, "edges");
        assert_eq!(
            edit.value,
            json::read(br#"["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge9"]"#).unwrap()
        );
        assert_eq!(
            told,
            "Inside corner no longer names the broken edge. Undo puts the broken reference back."
        );
        let last = fillet(&["k7f2qx3m.box.Edge5"]);
        assert_eq!(
            removed(
                &broken(0, "k7f2qx3m.box.Edge5"),
                &last,
                &read,
                "Inside corner"
            )
            .unwrap_err(),
            "Inside corner names no other edge, and a fillet names at least one: use a \
             candidate in its place, or suppress or delete Inside corner"
        );
        let chamfer = Dressing {
            chamfer: true,
            ..last
        };
        assert_eq!(
            chamfer.removable("Bevel").unwrap_err(),
            "Bevel names no other edge, and a chamfer names at least one: use a candidate in \
             its place, or suppress or delete Bevel"
        );
    }

    #[test]
    fn what_a_repair_says_tells_how_its_taken_back_where_it_was_made() {
        // On the document, Undo; in the feature's dialog, where Undo is off, its Cancel, and
        // after its OK one Undo for all of the dialog's edits.
        assert_eq!(
            used_words("Edge break", true, Made::Document),
            "Edge break now bevels the edge you picked. Undo puts the broken reference back."
        );
        assert_eq!(
            used_words("Inside corner", false, Made::Dialog),
            "Inside corner now rounds the edge you picked. Cancel puts the broken reference \
             back, as Undo does once the dialog is OK'd."
        );
        assert_eq!(
            removed_words("Inside corner", Made::Dialog),
            "Inside corner no longer names the broken edge. Cancel puts the broken reference \
             back, as Undo does once the dialog is OK'd."
        );
    }
}
