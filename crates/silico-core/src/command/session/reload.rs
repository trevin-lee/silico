//! The session's own reload of its file (A3, docs/design/m5.0-app.md, section 4): what its
//! disk watch tells it when a comparison finds the file changed and settled ([`reloader`]),
//! whether that queues a reload ([`Session::reload_wanted`]), the reload itself, a job on
//! the session's queue behind its commands ([`reload_job`], [`Session::reload_settled`]),
//! and what it came to ([`Session::reloaded_itself`]), a refusal kept ([`Refused`]) so the
//! same refusal isn't queued at each comparison (#479, #490). The file is read and taken as
//! a command's reload reads and takes it (decision 2: `Session::reload`, in `transaction`).

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use super::registry::{FileId, Registry, Unclaimed, claim, file_at};
use super::transaction::{Claim, Reload, Rereads, leads_to, messages};
use super::{Job, Problems, RunningCommand, Session, Ticket, Turn, command_span};
use crate::disk::Digest;
use crate::kernel::{Clients, Kernel};
use crate::worker::lock;

/// A session's own reloads (A3): one queued, and what the last refused, if it was refused
/// and none has been taken since.
#[derive(Default)]
pub(super) struct Reloads {
    pub(super) queued: bool,
    pub(super) refused: Option<Refused>,
}

/// The session's own reload, refused (a file another session holds, say), kept so the same
/// reload isn't queued at each comparison only to be refused again: the digest of the bytes
/// it was queued for (`digest`); what its claim was given of the file that held them, where
/// the session's path led and the file there (`file`); and the registry's count of claims
/// then (`claims`, `Book::claims`). It stands while each of those does, since those are all
/// a claim decides by: a file another session claimed may be taken once its claim changes,
/// and the same bytes in another file put at the path (the session's own, in place of a
/// link to another session's) may be taken at once (#490).
pub(super) struct Refused {
    digest: Digest,
    file: FoundFile,
    claims: u64,
}

/// What a reload's claim is given of the session's file besides its path as `open` resolved
/// it (fixed while the session holds the file): where that path leads now ([`leads_to`]),
/// and the file there, by device and inode, none if that couldn't be told. With the
/// registry's claims, all a claim decides by ([`claim`]).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct FoundFile {
    pub(super) found_at: PathBuf,
    pub(super) file: Option<FileId>,
}

/// What the disk watch's own reload came to ([`Session::reload_settled`]; A3, #479).
pub(super) enum SettledReload {
    /// Found changed, the settled bytes, and reloaded at a new revision.
    Reloaded,
    /// Nothing to change: the file as the session last read or wrote it (a command ahead of
    /// the reload reloaded it first), the finding stale.
    AsHeld,
    /// Nothing reloaded, and nothing kept: inside a transaction (a `begin` queued ahead of
    /// it), where the settled bytes or the session's are claimed, as `commit` claims what it
    /// reads, but nothing is reloaded; or the file found other than the settled bytes or the
    /// session's (changed again since it settled, perhaps mid-write; gone; unreadable;
    /// leading out of the workspace), not claimed; or the session's own bytes refused
    /// (another session's copy of them, through a link). The finding as the last comparison
    /// left it.
    Nothing,
    /// The settled bytes, refused (a file another session holds, no revision left to
    /// give), with what the claim was given of the file that held them: kept, so the same
    /// bytes aren't queued again while that file stays at the path and no claim changes.
    Refused(Problems, FoundFile),
}

impl<C: Kernel + 'static> Session<C> {
    /// Whether a comparison that found the file changed and settled, to bytes of digest
    /// `settled`, should queue the session's own reload (A3): not inside a transaction (only
    /// `commit` reads the file there), nor while its own save has left the file part written
    /// (the next save says where its old text is), nor while one is queued, nor for what a
    /// reload refused last ([`Refused`]: a file another session holds, say), the same bytes
    /// in the same file at the same path, at the same `claims`, which would be refused
    /// again; nor for a comparison made against `counter`, a count of the session's reads
    /// and writes no longer its own (it read or wrote since: dropped, never reported). If
    /// it should, it's marked queued.
    fn reload_wanted(&self, settled: &Digest, claims: u64, counter: u64) -> bool {
        {
            let held = self.held.read();
            let part_written = held
                .file
                .as_ref()
                .is_some_and(|file| file.part_written.is_some());
            if held.transaction.is_some() || part_written {
                return false;
            }
        }
        // Where the path leads now and the file there, as a reload's claim would be given
        // them: looked up with no lock held, since it opens the file.
        let now = self.found_now();
        let mut reloads = lock(&self.reloads);
        // A comparison the session has read or written since (its own reload, taking these
        // bytes) is dropped, never reported (the design's section 4): checked under the
        // reloads' lock, as a reload moves the watch on before it clears `queued` there, so
        // a comparison found current here either finds `queued` still set or is current.
        if !self.disk.get().is_some_and(|disk| disk.current(counter))
            || reloads.queued
            || reloads.refused.as_ref().is_some_and(|refused| {
                refused.digest == *settled
                    && refused.claims == claims
                    && now.as_ref() == Some(&refused.file)
            })
        {
            return false;
        }
        reloads.queued = true;
        true
    }

    /// What the session's own reload came to, queued for the bytes of digest `settled`
    /// ([`Session::reload_settled`]): reloaded at a new revision; the file found as the
    /// session holds it (a `begin` or another command ahead of it reloaded it first), the
    /// finding then `unchanged`; nothing, the finding as the last comparison left it; or
    /// the settled bytes refused, kept ([`Refused`]: `settled`, the file that held them, and
    /// `claims`) so the same bytes in the same file aren't queued again until a claim
    /// changes. Any other answer forgets a refusal. Whether the client should be woken.
    fn reloaded_itself(&self, reloaded: SettledReload, settled: Digest, claims: u64) -> bool {
        let (woken, refused) = match reloaded {
            SettledReload::Reloaded => (true, None),
            SettledReload::AsHeld => {
                let held = self.held.read();
                if let Some(file) = &held.file
                    && file.part_written.is_none()
                    && let Some(disk) = self.disk.get()
                {
                    // The file as the session holds it: the finding was stale.
                    disk.held(Arc::clone(&held.bytes));
                }
                (true, None)
            }
            SettledReload::Nothing => (false, None),
            SettledReload::Refused(problems, file) => {
                tracing::warn!(
                    session = %self.id,
                    "the file changed on disk, and the session couldn't reload it: {:?}",
                    problems.errors()
                );
                let refused = Refused {
                    digest: settled,
                    file,
                    claims,
                };
                (false, Some(refused))
            }
        };
        let mut reloads = lock(&self.reloads);
        reloads.queued = false;
        // Any reload taken forgets a refusal: its bytes may come back, to be tried again.
        reloads.refused = refused;
        woken
    }

    /// The disk watch's own reload (A3), queued for the bytes of digest `settled`, which it
    /// found settled (#479): the file read as `commit` reads it ([`Rereads::Always`]), inside
    /// a transaction too (a `begin` queued ahead of it), and taken as [`reload`](Self::reload)
    /// takes it only if it holds those bytes or the session's own. Anything else it finds
    /// (other bytes, changed again since it settled, perhaps mid-write; the file gone or
    /// unreadable; a path leading out of the workspace) isn't claimed, and nothing of it is
    /// kept: the watch reads it again, and its reload is queued once it has settled again.
    /// A refusal is kept only of the settled bytes themselves, with what its claim was given
    /// of the file that held them ([`FoundFile`], #490).
    fn reload_settled(
        &self,
        settled: &Digest,
        claim: &dyn Fn(Claim<'_>) -> Result<(), Unclaimed>,
    ) -> SettledReload {
        let reread = match self.reread(Rereads::Always) {
            Ok(Some(reread)) => reread,
            Ok(None) => return SettledReload::AsHeld,
            // Out of the workspace: refused before anything was read.
            Err(problems) => {
                tracing::info!(session = %self.id, "the file's reload read nothing, so nothing of it is kept: {}", messages(&problems));
                return SettledReload::Nothing;
            }
        };
        let read_settled = matches!(&reread.read, Ok((_, now)) if Digest::of(now) == *settled);
        let read_held = matches!(&reread.read, Ok((_, now)) if *now == *reread.held);
        if !read_settled && !read_held {
            tracing::info!(session = %self.id, "the file changed again before its reload read it: not reloaded until it settles");
            return SettledReload::Nothing;
        }
        // What the claim is given of the file, kept with a refusal of the settled bytes.
        let file = FoundFile {
            found_at: reread.found_at.clone(),
            file: match &reread.read {
                Ok((opened, _)) => FileId::of(opened).ok(),
                Err(_) => None,
            },
        };
        match self.reload_from(reread, claim) {
            Ok(Reload::Reloaded) => SettledReload::Reloaded,
            Ok(Reload::AsHeld) => SettledReload::AsHeld,
            // Inside a transaction: nothing reloaded.
            Ok(Reload::Outside(_)) => SettledReload::Nothing,
            // The settled bytes refused (another session's file, no revision left to give).
            Err(problems) if read_settled => SettledReload::Refused(problems, file),
            // The session's own bytes refused (another session's copy of them, through a
            // link): not what settled, so nothing is kept.
            Err(problems) => {
                tracing::info!(session = %self.id, "the file's reload found the session's own text and couldn't take it, so nothing of it is kept: {}", messages(&problems));
                SettledReload::Nothing
            }
        }
    }

    /// What a reload's claim would be given of the session's file now, as
    /// [`reread`](Self::reread) finds it, the file opened but not read: what a refusal kept
    /// of it is matched against ([`Session::reload_wanted`], #490). None for a session with
    /// no file, or whose path leads out of the workspace now, which a reload refuses before
    /// it reads anything, keeping nothing.
    fn found_now(&self) -> Option<FoundFile> {
        let resolved = self.held.read().file.as_ref()?.resolved.clone();
        let found_at = leads_to(self.workspace.as_ref(), &resolved)?;
        Some(FoundFile {
            found_at,
            file: file_at(&resolved),
        })
    }
}

/// What a session's disk watch tells it when a comparison finds its file changed and
/// settled: the session's own reload queued on its thread, behind its commands, if one is
/// wanted ([`Session::reload_wanted`]), as the next command would reload it (A3). Holds
/// both weakly, so the watch keeps neither alive.
pub(super) fn reloader<P: Clients + 'static>(
    registry: Weak<Registry<P>>,
    session: Weak<Session<P::Client>>,
) -> crate::disk::Changed {
    Arc::new(move |settled: &Digest, counter: u64| {
        let Some(session) = session.upgrade() else {
            return;
        };
        let claims = registry
            .upgrade()
            .map_or(0, |registry| lock(&registry.book).claims);
        if session.reload_wanted(settled, claims, counter) {
            session.queue(reload_job(Weak::clone(&registry), *settled));
        }
    })
}

/// The session's own reload, a job on its queue (A3): the file read as `commit` reads it,
/// and taken only if it holds `settled`, the digest of the bytes the disk watch found settled,
/// or the session's own ([`Session::reload_settled`]): then claimed as every command's read
/// is, and outside a transaction reloaded at a new revision. Anything else it finds changed
/// again since, perhaps mid-write, and is neither claimed nor kept: the watch reads it again
/// (#479). Named `reload` in `status.running` and the session's logs while it runs, and the
/// client woken by what it changed. A session closed first never runs it.
fn reload_job<P: Clients + 'static>(
    registry: Weak<Registry<P>>,
    settled: Digest,
) -> Job<P::Client> {
    Box::new(move |turn| {
        let Turn::Run(session) = turn else {
            return;
        };
        let stop = Ticket::reloading();
        *lock(&session.command) = Some(Arc::clone(&stop));
        let woken = {
            let _span = command_span(
                &session.logs,
                session.id,
                RunningCommand::Reload.as_str(),
                None,
            )
            .entered();
            #[cfg(test)]
            session.reloading.pass();
            let id = session.id;
            let claimed = |found: Claim<'_>| match registry.upgrade() {
                Some(registry) => {
                    claim(&registry, found.opened, found.resolved, found.found_at, id)
                }
                // The sessions are being dropped: no claim is kept.
                None => Ok(()),
            };
            // The claims as the reload found them: a refusal is kept until they change.
            let claims = registry
                .upgrade()
                .map_or(0, |registry| lock(&registry.book).claims);
            let reloaded = session.reload_settled(&settled, &claimed);
            let woken = session.reloaded_itself(reloaded, settled, claims);
            // A drag ends at any new revision.
            session.end_stale_drag();
            woken
        };
        lock(&session.command).take();
        if woken {
            session.waker.wake();
        }
    })
}
