//! The sketch edits that add or remove what the person wrote (docs/design/m5.0-app.md,
//! section 7, *The sketch edits*): `sketch_add` and `sketch_remove`, each an edit of the
//! session's document made against a revision, as `set_field` is ([`Session::edit`]): held
//! inside a transaction, on its undo stack; committed as its own transaction outside one.
//!
//! Neither solves the sketch, nor is refused for what its solve finds (SD7): once the gate
//! takes the edit, it's applied, and the rebuild's verdict, a conflict, a redundancy, or no
//! solution, is the sketch's state, which `check` reports and its features fail by.
//!
//! [`Session::edit`]: super::Session

use std::sync::Arc;

use super::{NewMembers, Running, SessionCommand, SessionId, Sessions, SketchEdited, reclaim};
use crate::command::Revision;
use crate::command::edit::{self, SketchChanges};
use crate::document::{FeatureId, Reference};
use crate::kernel::Clients;

impl<P: Clients + 'static> Sessions<P> {
    /// Adds `members` to `session`'s sketch `sketch` (the design's `sketch_add`), as an edit
    /// made against `revision`: each list as the document writes a sketch's, each member
    /// with its own ID, the batch checked whole by the gate before any of it is added. Not
    /// refused for a conflict or a redundancy it makes (SD7): the sketch's solve then reads
    /// so. An empty batch changes nothing: no new revision, nothing written. Refused whole,
    /// the session as it was, as every edit is ([`set_field`](Self::set_field)), for a
    /// feature that isn't a sketch (naming `sketch`, the document's sketches as
    /// candidates), and with every problem the reader or the gate finds in the members, each
    /// at its ID path (`circles.cb000003.radius`).
    pub fn sketch_add(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        members: NewMembers,
    ) -> Running<SketchEdited> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(
            session,
            SessionCommand::SketchAdd,
            move |session, stop, model| {
                let _span = tracing::info_span!("sketch_add", sketch = sketch.as_str()).entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| edit::sketch_add(base.checked()?.document(), &sketch, members),
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_edited(sketch, edited))
            },
        )
    }

    /// Removes `members` from `session`'s sketch `sketch` (the design's `sketch_remove`;
    /// sketch-edit decision 7), as an edit made against `revision`: a constraint; a curve,
    /// with each of its points no curve uses after the edit, and every constraint naming
    /// what goes; a point only if no curve uses it after the edit. The set is judged whole,
    /// duplicates taken once. Not refused for what the sketch's solve then finds (SD7). An
    /// empty list changes nothing. Refused as [`sketch_add`](Self::sketch_add) is, and for a
    /// member the sketch hasn't, a fixed reference (`origin`, `x_axis`, `y_axis`), and a
    /// point a curve still uses, each at its place in `members`.
    pub fn sketch_remove(
        &self,
        session: &SessionId,
        revision: Revision,
        sketch: FeatureId,
        members: Vec<Reference>,
    ) -> Running<SketchEdited> {
        let registry = Arc::downgrade(&self.registry);
        self.queued(
            session,
            SessionCommand::SketchRemove,
            move |session, stop, model| {
                let _span = tracing::info_span!(
                    "sketch_remove",
                    sketch = sketch.as_str(),
                    members = members.len()
                )
                .entered();
                let id = session.id;
                let edited = session.edit(
                    stop,
                    revision,
                    |base| edit::sketch_remove(base.checked()?.document(), &sketch, &members),
                    model,
                    &|resolved| reclaim(&registry, id, resolved),
                )?;
                Ok(sketch_edited(sketch, edited))
            },
        )
    }
}

/// A sketch edit's result: what it changed, as the edit found it (a commit's save changes no
/// member's ID, only a solve's coordinates), whether the document changed, and the commit.
fn sketch_edited(sketch: FeatureId, edited: super::edits::Edited<SketchChanges>) -> SketchEdited {
    let SketchChanges {
        added,
        modified,
        removed,
    } = edited.made;
    SketchEdited {
        sketch,
        added,
        modified,
        removed,
        changed: edited.changed,
        commit: edited.commit,
    }
}
