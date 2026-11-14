//! #479: a session's file written in place by another program, as `std::fs::write` writes
//! one (truncated as it's opened, then written through the handle), read by the disk watch
//! between the steps. A change is reloaded only once two comparisons in a row have found the
//! same bytes, and the session's own reload loads only those bytes, so a file the watch reads
//! empty, half written, or still growing never becomes the session's document and makes no
//! revision of its own; a file that stays as written, a broken one too, is reloaded, and its
//! problems reported, at the comparison after the one that found it. A command still reads
//! the file as it is when it runs (decision 2), half written too; nothing writes what it read
//! back over the finished file.
//!
//! Each test writes in steps through one handle and runs each comparison itself
//! ([`Watched::compare`]: the clock moved a whole interval, the comparison waited for until
//! it has ended and told what it tells), so which bytes each comparison reads is the test's,
//! never the scheduler's. Expected values: the bytes the tests write, the revisions counted
//! by hand from the complete writes, and RFC 8259 (a JSON object ends with its closing
//! brace, and Silico's reader takes only whitespace after it).

use super::*;
use std::io::Write as _;

/// Another program's write of a file in place: the file truncated as it's opened, then
/// written through the handle, in as many parts as the test writes.
struct InPlace(std::fs::File);

impl InPlace {
    /// The file at `path` opened for writing and truncated, as `std::fs::write` opens it.
    fn truncating(path: &Path) -> Self {
        InPlace(
            std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)
                .unwrap(),
        )
    }

    /// `part` written at the handle's end, unbuffered: on disk for the next read.
    fn write(&mut self, part: &str) {
        self.0.write_all(part.as_bytes()).unwrap();
    }
}

impl Watched {
    /// How many times a comparison has told the session the file changed and settled.
    fn told(&self) -> usize {
        self.f.session().disk.get().unwrap().changed_told()
    }

    /// The session at `revision`, with no reload queued or running and none told since the
    /// count was `told`, its file found as `disk`.
    #[track_caller]
    fn holds(&self, revision: Revision, told: usize, disk: Disk) {
        let status = self.f.status();
        assert_eq!(self.f.at(), revision);
        assert!(status.running.is_none(), "{status:?}");
        assert_eq!(status.disk, Some(disk));
        assert!(!lock(&self.f.session().reloads).queued);
        assert_eq!(self.told(), told);
    }

    /// The text of `revision`, as the session holds it, read without reading the file
    /// (a revision by number is never a reread, so nothing here reloads).
    fn text_of(&self, revision: Revision) -> String {
        self.f
            .sessions
            .text_at(&self.f.id, None, Some(Named::Revision(revision)))
            .wait()
            .result
            .unwrap()
            .text
    }

    /// Waits for the session's own reload to `revision`, then runs two comparisons more:
    /// the file as it holds it, no other reload. Its text then.
    #[track_caller]
    fn reloaded_once_to(&self, revision: Revision) -> String {
        self.f
            .status_reaches(|status, at| at == revision && status.running.is_none());
        let told = self.told();
        for _ in 0..2 {
            self.compare();
            self.holds(revision, told, Disk::Unchanged);
        }
        self.text_of(revision)
    }
}

/// A [`Watched`] session on [`BOX`], its first comparison run: the file unchanged.
fn watching_box() -> Watched {
    let w = watched(BOX);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Unchanged);
    w
}

#[test]
fn a_file_read_empty_between_its_truncate_and_its_write_is_reloaded_once() {
    let w = watching_box();
    let mut writer = InPlace::truncating(&w.f.path);
    assert_eq!(w.f.file(), "");
    // Read empty: changed, as it is, but told to no one, so nothing is reloaded.
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    assert_eq!(w.text_of(Revision::OPENED), BOX);
    writer.write(SPHERE);
    // Read whole: other bytes than the empty file the last read, so not yet told.
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    // Read whole again: settled, and reloaded, once.
    w.compare();
    assert_eq!(w.told(), 1);
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
    // The one revision it made replaced the opened one: no empty one between.
    assert_eq!(w.text_of(Revision::OPENED), BOX);
    assert_eq!(w.f.held_text(), SPHERE);
    assert_eq!(w.f.file(), SPHERE);
}

#[test]
fn a_file_read_half_written_is_reloaded_once_when_its_write_ends() {
    let w = watching_box();
    let (head, tail) = SPHERE.split_at(SPHERE.len() / 2);
    assert!(crate::document::read(head.as_bytes()).is_err());
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(head);
    // Half written, read twice in a row would be settled: read once, then the rest
    // written before the next.
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    writer.write(tail);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
    assert_eq!(w.text_of(Revision::OPENED), BOX);
}

#[test]
fn a_complete_document_that_isnt_json_and_stays_so_is_reloaded_at_the_next_comparison_and_reported()
{
    let w = watching_box();
    // Whole, but not JSON: a comma doubled.
    let broken = BOX.replace("\"length\": 40,", "\"length\": 40,,");
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(&broken);
    drop(writer);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    // The same bytes again: settled, and reloaded as it is, never held back.
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), broken);
    let status = w.f.status();
    assert!(!status.gated);
    assert_eq!(status.features, None);
    let problems =
        w.f.sessions
            .check(&w.f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems;
    assert_eq!(problems.errors().len(), 1);
    assert!(
        problems.errors()[0]
            .message()
            .starts_with("the document isn't valid JSON: "),
        "{problems:?}"
    );
    // Reported, not loaded again: the check read the file as the session holds it.
    assert_eq!(w.f.at(), Revision(2));
}

#[test]
fn a_complete_document_the_gate_refuses_and_stays_so_is_reloaded_at_the_next_comparison_and_reported()
 {
    let w = watching_box();
    let flat = BOX.replace("\"length\": 40", "\"length\": 0");
    InPlace::truncating(&w.f.path).write(&flat);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), flat);
    assert!(!w.f.status().gated);
    let problems =
        w.f.sessions
            .check(&w.f.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
            .problems;
    assert_eq!(problems.errors()[0].parameter(), Some("length"));
}

#[test]
fn a_documents_one_valid_strict_prefix_its_text_short_of_the_last_newline_is_never_loaded() {
    // A JSON object ends with its closing brace, after which the reader takes only
    // whitespace: a strict prefix of a document reads as one only when what's still to
    // come is whitespace. That prefix is a document, the same one; read once, it's still
    // not reloaded on its own.
    let w = watching_box();
    let whole = format!("{SPHERE}\n");
    assert!(crate::document::read(SPHERE.as_bytes()).is_ok());
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(SPHERE);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    writer.write("\n");
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), whole);
}

#[test]
fn a_file_growing_for_a_long_time_is_reloaded_once_when_it_stops() {
    let w = watching_box();
    // Written a few bytes at a time, each part read once before the next is written: as
    // a slow copy or a writer that flushes as it goes.
    let mut writer = InPlace::truncating(&w.f.path);
    let bytes = SPHERE.as_bytes();
    for part in bytes.chunks(7) {
        writer.write(std::str::from_utf8(part).unwrap());
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Changed);
    }
    assert_eq!(w.f.file(), SPHERE);
    // The last part read twice: settled.
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
}

#[test]
fn a_file_deleted_mid_write_is_gone_never_reloaded_and_a_complete_one_put_back_is_reloaded_once() {
    let w = watching_box();
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(&SPHERE[..10]);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    // Removed while the writer still holds it, and written on: to a file no path names.
    std::fs::remove_file(&w.f.path).unwrap();
    writer.write(&SPHERE[10..]);
    for _ in 0..3 {
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Gone);
    }
    // A command reads it too, and is refused: the session as it was.
    let refused = w.f.sessions.text(&w.f.id, None).wait();
    assert!(refused.result.is_err());
    assert_eq!(w.f.at(), Revision::OPENED);
    // The part read before the removal is forgotten: the same bytes put back at the path
    // are read as new, and settle only at the comparison after.
    std::fs::write(&w.f.path, &SPHERE[..10]).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    // A complete one put back, by rename, as Silico's own saves write.
    let beside = w.f.path.with_file_name("a.silico.new");
    std::fs::write(&beside, SPHERE).unwrap();
    std::fs::rename(&beside, &w.f.path).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
}

#[test]
fn a_file_replaced_by_rename_mid_write_is_reloaded_once_as_renamed() {
    let w = watching_box();
    let mut writer = InPlace::truncating(&w.f.path);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    // Another program's complete file renamed over it: the first writer goes on, into the
    // file it opened, which no path names any more.
    let beside = w.f.path.with_file_name("a.silico.new");
    let other = BOX.replace("Block", "Brick");
    std::fs::write(&beside, &other).unwrap();
    std::fs::rename(&beside, &w.f.path).unwrap();
    writer.write(&SPHERE[..20]);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    writer.write(&SPHERE[20..]);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), other);
    assert_eq!(w.f.file(), other);
}

#[test]
fn a_file_written_again_while_its_reload_waits_is_never_loaded_part_written() {
    let w = watching_box();
    let (reload_started, through) = shut(&w.f.session().reloading);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.compare();
    // Settled, and the reload queued: it waits at its gate, before it reads.
    assert_eq!(w.told(), 1);
    reload_started.recv_timeout(WAIT).unwrap();
    // Another write begun meanwhile: the file half written as the reload reads it.
    let other = BOX.replace("Block", "Brick");
    let (head, tail) = other.split_at(other.len() / 2);
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(head);
    open_gate(&w.f.session().reloading);
    through.send(()).unwrap();
    // Neither the bytes the watch found nor the session's: not loaded, nor refused.
    w.f.status_reaches(|status, _| {
        status.running.is_none() && !lock(&w.f.session().reloads).queued
    });
    w.holds(Revision::OPENED, 1, Disk::Changed);
    assert!(lock(&w.f.session().reloads).refused.is_none());
    assert_eq!(w.text_of(Revision::OPENED), BOX);
    // The half written file read by the watch: other bytes than it settled on.
    w.compare();
    w.holds(Revision::OPENED, 1, Disk::Changed);
    writer.write(tail);
    w.compare();
    w.holds(Revision::OPENED, 1, Disk::Changed);
    w.compare();
    assert_eq!(w.told(), 2);
    assert_eq!(w.reloaded_once_to(Revision(2)), other);
}

#[test]
fn a_file_rewritten_in_place_with_the_bytes_it_held_reloads_nothing() {
    let w = watching_box();
    let mut writer = InPlace::truncating(&w.f.path);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    writer.write(BOX);
    for _ in 0..3 {
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Unchanged);
    }
}

#[test]
fn a_change_is_reloaded_only_once_two_comparisons_in_a_row_find_the_same_bytes() {
    let w = watching_box();
    let brick = BOX.replace("Block", "Brick");
    // Sphere, brick, sphere: no two in a row the same.
    for text in [SPHERE, brick.as_str(), SPHERE] {
        std::fs::write(&w.f.path, text).unwrap();
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Changed);
    }
    // Sphere, gone, sphere: what's between is no change, but breaks the row too.
    std::fs::remove_file(&w.f.path).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Gone);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    // Sphere twice in a row.
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
}

#[test]
fn a_commit_while_the_file_is_half_written_is_a_conflict_and_writes_nothing() {
    let w = watching_box();
    w.f.begin();
    let set =
        w.f.sessions
            .set_field(
                &w.f.id,
                Revision::OPENED,
                id("k7f2qx3m"),
                "length".to_owned(),
                length("41"),
            )
            .wait();
    assert!(set.result.unwrap().changed);
    let (head, tail) = SPHERE.split_at(SPHERE.len() / 2);
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(head);
    // The commit reads the file itself: half written, not what the transaction began on.
    let refused = w.f.sessions.commit(&w.f.id).wait().result.unwrap_err();
    assert_eq!(refused.errors()[0].code(), ErrorCode::Conflict);
    assert_eq!(w.f.file(), head);
    writer.write(tail);
    assert_eq!(w.f.file(), SPHERE);
    // Rolled back (the app's Reload): the complete file is reloaded, once it has settled.
    let rolled = w.f.sessions.rollback(&w.f.id).wait();
    let at = rolled.session.unwrap().revision;
    w.compare();
    w.compare();
    let next = Revision(at.0 + 1);
    assert_eq!(w.reloaded_once_to(next), SPHERE);
}

#[test]
fn a_command_sent_mid_write_reads_the_file_as_it_is_and_what_it_loaded_is_never_written_back() {
    // A command reads the file itself before it runs (decision 2), whatever the watch has
    // found: one sent while the file is half written reloads it as it is, its problems
    // reported, as for any file that doesn't read. The watch then reloads the complete file
    // once it settles. Nothing writes the half written text back: an edit is refused as it
    // was sent at the revision the watch's reload replaced, and the text itself, as a
    // Restore of that revision would put back, doesn't read, so `replace_text` refuses it.
    let w = watching_box();
    let (head, tail) = SPHERE.split_at(SPHERE.len() / 2);
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(head);
    let read = w.f.sessions.text(&w.f.id, None).wait();
    let session = read.session.unwrap();
    assert!(session.reloaded);
    assert_eq!(session.revision, Revision(2));
    assert_eq!(read.result.unwrap().text, head);
    assert!(!w.f.status().gated);
    writer.write(tail);
    w.compare();
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(3)), SPHERE);
    let set =
        w.f.sessions
            .set_field(
                &w.f.id,
                Revision(2),
                id("a91cw0de"),
                "radius".to_owned(),
                length("6"),
            )
            .wait();
    assert_eq!(
        set.result.unwrap_err().errors()[0].code(),
        ErrorCode::Conflict
    );
    let restore = w.text_of(Revision(2));
    assert_eq!(restore, head);
    w.f.begin();
    let replaced =
        w.f.sessions
            .replace_text(&w.f.id, Revision(3), restore)
            .wait();
    assert!(replaced.result.is_err());
    assert_eq!(w.f.file(), SPHERE);
}

#[test]
fn a_file_removed_while_its_reload_waits_is_neither_loaded_nor_refused_and_is_reloaded_put_back() {
    // Before #479 the reload refused a file gone, keeping the bytes it was queued for as
    // refused, so the same bytes put back were never reloaded by the watch again.
    let w = watching_box();
    let (reload_started, through) = shut(&w.f.session().reloading);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.compare();
    reload_started.recv_timeout(WAIT).unwrap();
    std::fs::remove_file(&w.f.path).unwrap();
    open_gate(&w.f.session().reloading);
    through.send(()).unwrap();
    w.f.status_reaches(|status, _| {
        status.running.is_none() && !lock(&w.f.session().reloads).queued
    });
    assert!(lock(&w.f.session().reloads).refused.is_none());
    w.holds(Revision::OPENED, 1, Disk::Changed);
    w.compare();
    w.holds(Revision::OPENED, 1, Disk::Gone);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 1, Disk::Changed);
    w.compare();
    assert_eq!(w.told(), 2);
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
}

#[test]
fn a_write_read_once_cut_at_every_byte_offset_is_reloaded_once_whole() {
    // Each document written over the other in place, read once cut at each byte offset
    // (none, the whole, and every one between), then finished: one revision each, holding
    // the whole text.
    let w = watching_box();
    let longest = BOX.len().max(SPHERE.len());
    let mut at = Revision::OPENED;
    for cut in 0..=longest {
        for text in [SPHERE, BOX] {
            let (head, tail) = text.split_at(cut.min(text.len()));
            let told = w.told();
            let mut writer = InPlace::truncating(&w.f.path);
            writer.write(head);
            w.compare();
            w.holds(at, told, Disk::Changed);
            writer.write(tail);
            w.compare();
            if w.told() == told {
                // Read cut, then whole: not settled until whole twice.
                w.holds(at, told, Disk::Changed);
                w.compare();
            }
            assert_eq!(w.told(), told + 1, "cut at {cut} of {text}");
            at = Revision(at.0 + 1);
            assert_eq!(w.reloaded_once_to(at), text, "cut at {cut}");
        }
    }
}

#[test]
fn bytes_found_before_the_sessions_own_read_and_save_settle_only_at_two_comparisons_after() {
    // Two comparisons in a row against the same bytes of the session's: the bytes one found
    // before the session read or wrote its file are forgotten, so the same bytes found
    // again after are new.
    let w = watching_box();
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    // A command reads the file itself (revision 2), then an edit saves it (revision 3).
    let read = w.f.sessions.text(&w.f.id, None).wait();
    assert_eq!(read.session.unwrap().revision, Revision(2));
    let set =
        w.f.sessions
            .set_field(
                &w.f.id,
                Revision(2),
                id("a91cw0de"),
                "radius".to_owned(),
                length("6"),
            )
            .wait();
    assert!(set.result.unwrap().changed);
    assert_eq!(set.session.unwrap().revision, Revision(3));
    let saved = w.f.file();
    assert_ne!(saved, SPHERE);
    // The sphere put back, as the first comparison found it: found once since the save.
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.holds(Revision(3), 0, Disk::Changed);
    w.compare();
    assert_eq!(w.told(), 1);
    assert_eq!(w.reloaded_once_to(Revision(4)), SPHERE);
}
