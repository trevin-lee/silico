//! Sketch mode's selection (docs/design/m5.0-app.md, section 9, *Selection*, in sketch mode;
//! the constraint tools' C9, docs/study/constraint-tools.md): what's picked, in the order it
//! was picked, the members of the sketch and the origin and its axes alike. The constraint
//! tools read that order (C1's survivor, C4's mirror, C8's first). It's the person's own
//! input, so it's as deterministic as the clicks that made it. A click changes it by open
//! item 24's rules ([`SketchSelection::click`]).
//!
//! C9 also orders what joins together, as a box's would: in ID order (the members by ID,
//! then the origin and the axes), after what's already selected. Sketch mode has no box
//! yet; one that joins several at once follows that rule.

use std::collections::BTreeSet;

use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, ElementKind, Reference, Sketch};

use super::hit::{Hit, SketchPick};

/// What's selected in sketch mode, in pick order, each once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SketchSelection {
    picked: Vec<SketchPick>,
}

impl SketchSelection {
    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.picked.is_empty()
    }

    /// Whether `picked` is selected.
    pub fn contains(&self, picked: &SketchPick) -> bool {
        self.picked.contains(picked)
    }

    /// Nothing selected.
    pub fn clear(&mut self) {
        self.picked.clear();
    }

    /// A click on `picked` (none: empty space), with Ctrl or not: open item 24's rules, as
    /// SolidWorks' selection. A plain click selects only what it's on; a Ctrl-click takes it
    /// away if it's selected, and else adds it at the end, so one toggled back in comes
    /// last; a click on nothing clears the selection (as Esc does, the tab's).
    pub fn click(&mut self, picked: Option<SketchPick>, ctrl: bool) {
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

    /// The members selected, in pick order: the origin and the axes left out.
    pub fn members(&self) -> impl Iterator<Item = &Hit> {
        self.picked.iter().filter_map(SketchPick::member)
    }

    /// What's selected as the constraint tools take it, in pick order: each member by its
    /// ID, and the origin and the axes as themselves.
    pub fn references(&self) -> Vec<Reference> {
        self.picked.iter().map(SketchPick::reference).collect()
    }

    /// What `read` doesn't hold let go ([`SketchPick::held_by`]), the rest kept in their
    /// order: what a new read does to it. The origin and the axes, every sketch's, stay.
    pub fn retain_held(&mut self, read: &SketchRead) {
        let held: BTreeSet<&ElementId> = read.members.elements().map(|(id, _)| id).collect();
        self.picked
            .retain(|picked| picked.held_by(|id| held.contains(id)));
    }
}

/// What Construction (its button, or X) does to the curves selected (the approved
/// prototype's "Toggles the selected curves between profile and construction"): the value
/// they all take, and each field to set to it (`lines.<id>.construction`, and the circles'
/// and arcs' the same), for the curves that don't hold it already, in pick order: `first`,
/// then `rest`. There's always one, since the value is the one some curve doesn't hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructionSwitch {
    pub construction: bool,
    pub first: String,
    pub rest: Vec<String>,
}

/// Construction's switch of the curves among `picks`, each a line, circle, or arc of
/// `members`, once: all made construction, unless every one is already, when all are made
/// profile, as SolidWorks' *For construction* and Fusion's Construction set a mixed
/// selection one way. Points, constraints, the origin, and the axes among `picks` are left
/// out, and so is what `members` doesn't hold. None when no curve is picked: then the
/// palette's setting is what Construction turns.
pub fn construction_switch(members: &Sketch, picks: &[Reference]) -> Option<ConstructionSwitch> {
    let curve = |id: &ElementId| -> Option<(ElementKind, bool)> {
        let line = members.lines.iter().find(|line| &line.id == id);
        let circle = members.circles.iter().find(|circle| &circle.id == id);
        let arc = members.arcs.iter().find(|arc| &arc.id == id);
        line.map(|line| (ElementKind::Line, line.construction))
            .or_else(|| circle.map(|circle| (ElementKind::Circle, circle.construction)))
            .or_else(|| arc.map(|arc| (ElementKind::Arc, arc.construction)))
    };
    let mut seen = BTreeSet::new();
    let curves: Vec<(&ElementId, ElementKind, bool)> = picks
        .iter()
        .filter_map(|pick| match pick {
            Reference::Element(id) => Some(id),
            Reference::Origin | Reference::XAxis | Reference::YAxis => None,
        })
        .filter(|id| seen.insert(*id))
        .filter_map(|id| curve(id).map(|(kind, construction)| (id, kind, construction)))
        .collect();
    if curves.is_empty() {
        return None;
    }
    let construction = !curves.iter().all(|&(_, _, construction)| construction);
    let mut fields = curves
        .iter()
        .filter(|&&(_, _, held)| held != construction)
        .map(|(id, kind, _)| format!("{}.{id}.construction", kind.list()));
    let first = fields.next()?;
    Some(ConstructionSwitch {
        construction,
        first,
        rest: fields.collect(),
    })
}
