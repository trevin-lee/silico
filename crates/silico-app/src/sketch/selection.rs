//! Sketch mode's selection (docs/design/m5.0-app.md, section 9, *Selection*, in sketch mode;
//! the constraint tools' C9, docs/study/constraint-tools.md): what's picked, in the order it
//! was picked, the members of the sketch and the origin and its axes alike. The constraint
//! tools read that order (C1's survivor, C4's mirror, C8's first). It's the person's own
//! input, so it's as deterministic as the clicks that made it.
//!
//! Open item 24's rules: a plain click replaces the selection, a Ctrl-click toggles one
//! thing (one toggled back in joins at the end), and a click on nothing (or Esc, the tab's)
//! clears it. Several that join together, as a box's would, join in ID order, after what's
//! already selected.

use std::collections::BTreeSet;

use silico_core::document::{ElementId, Reference};

use super::hit::{Hit, Picked};

/// What's selected in sketch mode, in pick order, each once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    picked: Vec<Picked>,
}

impl Selection {
    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.picked.is_empty()
    }

    /// Whether `picked` is selected.
    pub fn contains(&self, picked: &Picked) -> bool {
        self.picked.contains(picked)
    }

    /// Nothing selected.
    pub fn clear(&mut self) {
        self.picked.clear();
    }

    /// A click on `picked` (none: empty space), with Ctrl or not (open item 24, as
    /// SolidWorks' selection): a plain click selects only what it's on; a Ctrl-click takes
    /// it away if it's selected, and else adds it at the end; a click on nothing clears the
    /// selection.
    pub fn click(&mut self, picked: Option<Picked>, ctrl: bool) {
        match (picked, ctrl) {
            (None, _) => self.picked.clear(),
            (Some(picked), false) => {
                self.picked.clear();
                self.picked.push(picked);
            }
            (Some(picked), true) => {
                let before = self.picked.len();
                self.picked.retain(|held| *held != picked);
                if self.picked.len() == before {
                    self.picked.push(picked);
                }
            }
        }
    }

    /// Several joining together, as a box's would (C9): each not already selected added after
    /// what is, in ID order ([`Picked`]'s: the members by ID, then the origin and the axes),
    /// each once. What's already selected keeps its place.
    pub fn add_together(&mut self, picked: impl IntoIterator<Item = Picked>) {
        let held: BTreeSet<Picked> = self.picked.iter().cloned().collect();
        let joining: BTreeSet<Picked> = picked.into_iter().filter(|p| !held.contains(p)).collect();
        self.picked.extend(joining);
    }

    /// The members selected, in pick order: the origin and the axes left out.
    pub fn members(&self) -> impl Iterator<Item = &Hit> {
        self.picked.iter().filter_map(Picked::member)
    }

    /// What's selected as the constraint tools take it, in pick order: each member by its
    /// ID, and the origin and the axes as themselves.
    pub fn references(&self) -> Vec<Reference> {
        self.picked.iter().map(Picked::reference).collect()
    }

    /// The members `held` doesn't hold let go, the rest kept in their order: what a new read
    /// does to it. The origin and the axes, every sketch's, stay.
    pub fn keep_members(&mut self, held: impl Fn(&ElementId) -> bool) {
        self.picked
            .retain(|picked| picked.member().is_none_or(|hit| held(&hit.id)));
    }
}
