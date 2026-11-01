//! What a tool has added that the newest read may not hold yet: each add with its token, the
//! members it made, and the revision the core answered it with, for the events after it to
//! find (a chain's last point, a point clicked twice) and for the IDs it gave to stay taken.
//! The sketch tools and Smart Dimension each keep one.

use std::collections::BTreeSet;

use super::{AddToken, Context};
use crate::command::Revision;
use crate::document::sketch_edit::Members;
use crate::document::{ElementId, FeatureId};

/// Each tool's serial, from one count for the process. Tokens never reach a request, a
/// result, a name, or the document, so the count's order (which thread made a tool first)
/// changes nothing that's written.
static TOOL_SERIALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A tool's adds the newest read may not hold. Not `Clone`: a copy would give the same
/// tokens as the one it copied.
#[derive(Debug)]
pub(super) struct Adds {
    /// The sketch the tool works in: a context of another starts it afresh.
    sketch: Option<FeatureId>,
    /// Each add in turn, with the revision it made once the core has answered.
    pending: Vec<Pending>,
    /// This tool's serial, which its tokens carry.
    serial: u64,
    /// The number the next add's token is given.
    next_token: u64,
}

/// One add the tool made: its members, until a read holds them or a read of the revision
/// it made (or later) doesn't (undone, rolled back, or removed by another client).
#[derive(Debug, Clone)]
struct Pending {
    token: AddToken,
    members: Members,
    revision: Option<Revision>,
}

impl Adds {
    pub(super) fn new() -> Self {
        Adds {
            sketch: None,
            pending: Vec::new(),
            serial: TOOL_SERIALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            next_token: 0,
        }
    }

    /// `members`, just made, taken to be in the sketch until a read settles them: the
    /// token the viewport answers the add by.
    pub(super) fn issue(&mut self, members: &Members) -> AddToken {
        let token = AddToken {
            tool: self.serial,
            add: self.next_token,
        };
        self.next_token += 1;
        self.pending.push(Pending {
            token,
            members: members.clone(),
            revision: None,
        });
        token
    }

    /// The core took the add `token` names, at `revision`.
    pub(super) fn added(&mut self, token: AddToken, revision: Revision) {
        if let Some(pending) = self.pending.iter_mut().find(|p| p.token == token) {
            pending.revision = Some(revision);
        }
    }

    /// The core refused the add `token` names: it's forgotten.
    pub(super) fn refused(&mut self, token: AddToken) {
        self.pending.retain(|p| p.token != token);
    }

    /// Every add forgotten.
    pub(super) fn clear(&mut self) {
        self.pending.clear();
    }

    /// The sketch the tool works in, once a context has named one.
    pub(super) fn sketch(&self) -> Option<&FeatureId> {
        self.sketch.as_ref()
    }

    /// The adds brought up to `context`'s read: every add forgotten if it's of another sketch
    /// than the last (whether so is the answer: the tool then starts afresh); those a read at
    /// or past their revision has settled forgotten, and what the read holds of the rest left
    /// to the read.
    pub(super) fn settle(&mut self, context: &Context<'_>) -> bool {
        let switched = self.sketch.as_ref() != Some(context.sketch_id);
        if switched {
            let afresh = self.sketch.is_some();
            self.sketch = Some(context.sketch_id.clone());
            if afresh {
                self.pending.clear();
            }
            self.forget_settled(context);
            return afresh;
        }
        self.forget_settled(context);
        false
    }

    fn forget_settled(&mut self, context: &Context<'_>) {
        self.pending
            .retain(|p| p.revision.is_none_or(|made| made > context.revision));
        if !self.pending.is_empty() {
            let held: BTreeSet<&ElementId> = context.sketch.elements().map(|(id, _)| id).collect();
            for p in &mut self.pending {
                let m = &mut p.members;
                m.points.retain(|x| !held.contains(&x.id));
                m.lines.retain(|x| !held.contains(&x.id));
                m.circles.retain(|x| !held.contains(&x.id));
                m.arcs.retain(|x| !held.contains(&x.id));
                m.constraints.retain(|x| !held.contains(&x.id));
            }
        }
    }

    /// The members of every add the read may not hold yet.
    pub(super) fn members(&self) -> Members {
        let mut all = Members::default();
        for p in &self.pending {
            let m = &p.members;
            all.points.extend(m.points.iter().cloned());
            all.lines.extend(m.lines.iter().cloned());
            all.circles.extend(m.circles.iter().cloned());
            all.arcs.extend(m.arcs.iter().cloned());
            all.constraints.extend(m.constraints.iter().cloned());
        }
        all
    }
}
