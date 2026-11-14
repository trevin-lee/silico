//! #479: a session's file written in place by another program, as `std::fs::write` writes
//! one (truncated as it's opened, then written through the handle), read by the disk watch
//! between the steps. A change is reloaded only once two comparisons in a row have found the
//! same bytes, never empty, and the session's own reload loads only those bytes, so a file
//! the watch reads empty, or half written or still growing at one comparison and otherwise
//! at the next, never becomes the session's document and makes no revision of its own; a
//! file that stays as written, a broken one too, is reloaded, and its problems reported, at
//! the comparison after the one that found it, and so is a part of a write left for longer
//! than an interval (the limit settling leaves). A command still reads the file as it is
//! when it runs (decision 2), half written too; nothing writes what it read back over the
//! finished file.
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
    fn told_count(&self) -> usize {
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
        assert_eq!(self.told_count(), told);
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
        let told = self.told_count();
        for _ in 0..2 {
            self.compare();
            self.holds(revision, told, Disk::Unchanged);
        }
        self.text_of(revision)
    }

    /// One comparison of this session's file, as [`compare`](Watched::compare) runs one, for
    /// a test whose other sessions share the reader (so its count of reads isn't this
    /// session's alone): the clock moved a whole interval and `status` asked, then waited
    /// for until a comparison has ended and told what it tells.
    #[track_caller]
    fn compare_with_shared_reader(&self) {
        let before = self.compared();
        self.clock.advance(DISK_CHECK_INTERVAL);
        self.f.status();
        let deadline = Instant::now() + WAIT;
        while self.compared() == before {
            assert!(Instant::now() < deadline, "no comparison ran");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Waits until the session's own reload, queued or running, has ended.
    #[track_caller]
    fn reload_ends(&self) {
        self.f.status_reaches(|status, _| {
            status.running.is_none() && !lock(&self.f.session().reloads).queued
        });
    }

    /// [`SPHERE`] put back at the session's path as a regular file, found at two comparisons
    /// in a row, told, and reloaded: revision 2, the sphere.
    #[track_caller]
    fn settled_put_back_is_reloaded(&self) {
        let _ = std::fs::remove_file(&self.f.path);
        std::fs::write(&self.f.path, SPHERE).unwrap();
        let told = self.told_count();
        self.compare_with_shared_reader();
        self.compare_with_shared_reader();
        assert!(self.told_count() > told, "the watch never told");
        self.reload_ends();
        assert_eq!(
            self.f.at(),
            Revision(2),
            "the sphere put back is never reloaded"
        );
        assert_eq!(self.text_of(Revision(2)), SPHERE);
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
    assert_eq!(w.told_count(), 1);
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
    // A command reads it too, and is refused, naming the file: the session as it was.
    let refused = w.f.sessions.text(&w.f.id, None).wait().result.unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("path"));
    assert_eq!(refused.errors()[0].cause(), Some(NOT_RELOADED));
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
    let count = counting(&w.f.sessions);
    let (reload_started, through) = shut(&w.f.session().reloading);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.compare();
    // Settled, and the reload queued: it waits at its gate, before it reads.
    assert_eq!(w.told_count(), 1);
    reload_started.recv_timeout(WAIT).unwrap();
    // Another write begun meanwhile: the file half written as the reload reads it.
    let other = BOX.replace("Block", "Brick");
    let (head, tail) = other.split_at(other.len() / 2);
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(head);
    let wakes = count.load(Ordering::SeqCst);
    open_gate(&w.f.session().reloading);
    through.send(()).unwrap();
    // Neither the bytes the watch found nor the session's: not loaded, nor refused, and the
    // client not woken, as nothing it draws changed. A read queued behind the reload, held
    // as it starts, starts only once the reload has returned, its wake given if it gave one.
    let (read_started, read_through) = shut(&w.f.session().gate);
    let read =
        w.f.sessions
            .text_at(&w.f.id, None, Some(Named::Revision(Revision::OPENED)));
    read_started.recv_timeout(WAIT).unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        wakes,
        "the reload woke the client"
    );
    open_gate(&w.f.session().gate);
    read_through.send(()).unwrap();
    assert_eq!(read.wait().result.unwrap().text, BOX);
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
    assert_eq!(w.told_count(), 2);
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
    // Outside a transaction, where an edit taken is saved at once: refused, nothing written.
    let replaced =
        w.f.sessions
            .replace_text(&w.f.id, Revision(3), restore)
            .wait()
            .result
            .unwrap_err();
    assert_eq!(replaced.errors()[0].parameter(), Some("text"));
    assert_eq!(w.f.at(), Revision(3));
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
    assert_eq!(w.told_count(), 2);
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
            let told = w.told_count();
            let mut writer = InPlace::truncating(&w.f.path);
            writer.write(head);
            w.compare();
            w.holds(at, told, Disk::Changed);
            writer.write(tail);
            w.compare();
            if w.told_count() == told {
                // Read cut, then whole: not settled until whole twice.
                w.holds(at, told, Disk::Changed);
                w.compare();
            }
            assert_eq!(w.told_count(), told + 1, "cut at {cut} of {text}");
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
    assert_eq!(w.told_count(), 1);
    assert_eq!(w.reloaded_once_to(Revision(4)), SPHERE);
}

#[test]
fn a_file_overwritten_in_place_without_truncation_is_reloaded_once_whole() {
    // Overwritten from its start, never truncated (as `dd conv=notrunc`, or an editor
    // writing through its handle): every read is the old document's length, the new text
    // over the old one's start.
    let w = watching_box();
    let new = BOX
        .replace("Block", "Brick")
        .replace("\"length\": 40", "\"length\": 41")
        .replace("\"width\": 20", "\"width\": 21");
    assert_eq!(new.len(), BOX.len());
    let m1 = new.find("Brick").unwrap() + "Brick".len();
    let m2 = new.find("\"length\": 41").unwrap() + "\"length\": 41".len();
    let mut writer = InPlace(
        std::fs::OpenOptions::new()
            .write(true)
            .open(&w.f.path)
            .unwrap(),
    );
    writer.write(&new[..m1]);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    writer.write(&new[m1..m2]);
    // Brick, 41, 20: neither document, yet one, and the last read's length.
    let mixed = w.f.file();
    assert!(crate::document::read(mixed.as_bytes()).is_ok());
    assert_eq!(mixed.len(), BOX.len());
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    writer.write(&new[m2..]);
    assert_eq!(w.f.file(), new);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), new);
}

#[test]
fn settled_bytes_put_back_after_another_sessions_file_was_linked_in_mid_reload_are_reloaded() {
    // The reload finds other bytes than settled, another session's, through a link: neither
    // claimed nor refused, so the settled bytes put back are reloaded.
    let w = watching_box();
    let other = w.f.path.with_file_name("b.silico");
    let brick = BOX.replace("Block", "Brick");
    std::fs::write(&other, &brick).unwrap();
    let _second = w.f.sessions.open(&other, false).wait().session.unwrap().id;
    let (reload_started, through) = shut(&w.f.session().reloading);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare_with_shared_reader();
    w.compare_with_shared_reader();
    reload_started.recv_timeout(WAIT).unwrap();
    // While the reload waits: the other session's file put at the path, through a link.
    std::fs::remove_file(&w.f.path).unwrap();
    std::os::unix::fs::symlink(&other, &w.f.path).unwrap();
    open_gate(&w.f.session().reloading);
    through.send(()).unwrap();
    w.reload_ends();
    assert_eq!(w.f.at(), Revision::OPENED);
    assert!(lock(&w.f.session().reloads).refused.is_none());
    w.settled_put_back_is_reloaded();
}

#[test]
fn two_in_place_saves_of_the_held_text_never_reload_the_empty_file_between() {
    // An editor saving the session's own text twice, each a truncate then a write: read
    // empty, then whole (unchanged), then empty again. Empty in two comparisons, but never
    // two in a row.
    let w = watching_box();
    let mut first = InPlace::truncating(&w.f.path);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    first.write(BOX);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Unchanged);
    let mut second = InPlace::truncating(&w.f.path);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    second.write(BOX);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Unchanged);
}

#[test]
fn two_in_place_saves_of_the_held_text_never_reload_the_part_between() {
    // The same, each save written in two parts: read half written, then whole (unchanged),
    // then half written again. The same changed bytes in two comparisons, but never two in
    // a row.
    let w = watching_box();
    let (head, tail) = BOX.split_at(BOX.len() / 2);
    for _ in 0..2 {
        let mut save = InPlace::truncating(&w.f.path);
        save.write(head);
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Changed);
        save.write(tail);
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Unchanged);
    }
}

#[test]
fn the_same_part_read_either_side_of_an_unreadable_file_is_never_settled() {
    // Half written, then not a file this user may read (a directory put at the path), then
    // half written again: the same changed bytes in two comparisons, but not in a row.
    let w = watching_box();
    let (head, _) = SPHERE.split_at(SPHERE.len() / 2);
    std::fs::write(&w.f.path, head).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    std::fs::remove_file(&w.f.path).unwrap();
    std::fs::create_dir(&w.f.path).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Unreadable);
    std::fs::remove_dir(&w.f.path).unwrap();
    std::fs::write(&w.f.path, head).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
}

#[test]
fn a_write_paused_past_an_interval_after_its_truncate_never_reloads_the_empty_file() {
    // A writer that truncates, then takes longer than an interval to write (a shell
    // redirect of a slow script, a copy over a slow link): the empty file is read at
    // comparison after comparison. It's never a document, so the watch never tells of it.
    let w = watching_box();
    let mut writer = InPlace::truncating(&w.f.path);
    for _ in 0..3 {
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Changed);
    }
    writer.write(SPHERE);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.told_count(), 1);
    assert_eq!(w.reloaded_once_to(Revision(2)), SPHERE);
    assert_eq!(w.text_of(Revision::OPENED), BOX);
}

#[test]
fn a_file_left_empty_stays_changed_and_a_command_reads_it_and_reports_the_readers_error() {
    // Never told, the empty file is still reported: `status.disk` says changed for as long
    // as it stays so, and a command reads it as it is (decision 2), its problems said, as
    // for any file that doesn't read.
    let w = watching_box();
    drop(InPlace::truncating(&w.f.path));
    for _ in 0..3 {
        w.compare();
        w.holds(Revision::OPENED, 0, Disk::Changed);
    }
    let read = w.f.sessions.text(&w.f.id, None).wait();
    let session = read.session.unwrap();
    assert!(session.reloaded);
    assert_eq!(session.revision, Revision(2));
    assert_eq!(read.result.unwrap().text, "");
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
    // The session holds the empty file now: unchanged, nothing more told.
    w.compare();
    w.holds(Revision(2), 0, Disk::Unchanged);
}

#[test]
fn a_part_left_written_for_more_than_an_interval_is_reloaded_then_the_whole_the_stated_limit() {
    // The limit #479 leaves: a writer that leaves a part of its text, not nothing, for
    // longer than an interval has it read the same at two comparisons in a row, settled,
    // and reloaded as it is, its problems reported, as a file that stayed broken; then the
    // whole, once it's written and settled.
    let w = watching_box();
    let (head, tail) = SPHERE.split_at(SPHERE.len() / 2);
    let mut writer = InPlace::truncating(&w.f.path);
    writer.write(head);
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(2)), head);
    assert!(!w.f.status().gated);
    writer.write(tail);
    w.compare();
    w.holds(Revision(2), 1, Disk::Changed);
    w.compare();
    assert_eq!(w.reloaded_once_to(Revision(3)), SPHERE);
}

#[test]
fn settled_bytes_put_back_after_a_link_out_of_the_workspace_are_reloaded() {
    // The reload finds the path leading out of the workspace: refused before anything is
    // read, so nothing is kept of the bytes the watch found settled.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let root = dir.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("a.silico");
    std::fs::write(&path, BOX).unwrap();
    let outside = dir.path().join("outside.silico");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let clock = TestClock::new();
    let reader = Arc::new(HeldReader::default());
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        || Ok(Box::new(crate::fake::FakeSketches::new()) as Box<dyn SketchSolver>),
    )
    .watching_disk(DISK_CHECK_INTERVAL, reader.reader(), clock.clock())
    .unwrap()
    .in_workspace(&root)
    .unwrap();
    let id = sessions.open(&path, false).wait().session.unwrap().id;
    let w = Watched {
        f: Fixture {
            _dir: dir,
            kernel,
            sessions,
            id,
            path,
        },
        clock,
        reader,
    };
    w.compare();
    w.holds(Revision::OPENED, 0, Disk::Unchanged);
    let (reload_started, through) = shut(&w.f.session().reloading);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.compare();
    assert_eq!(w.told_count(), 1);
    reload_started.recv_timeout(WAIT).unwrap();
    std::fs::write(&outside, BOX.replace("Block", "Brick")).unwrap();
    std::fs::remove_file(&w.f.path).unwrap();
    std::os::unix::fs::symlink(&outside, &w.f.path).unwrap();
    open_gate(&w.f.session().reloading);
    through.send(()).unwrap();
    w.reload_ends();
    // Nothing done: the finding as the last comparison left it, changed.
    w.holds(Revision::OPENED, 1, Disk::Changed);
    assert!(lock(&w.f.session().reloads).refused.is_none());
    w.settled_put_back_is_reloaded();
}

#[test]
fn settled_bytes_put_back_after_a_link_to_another_sessions_copy_of_the_held_text_are_reloaded() {
    // The reload finds the session's own text, but through a link to another session's
    // file: refused by the claim, for bytes that aren't the ones the watch found settled, so
    // nothing is kept of those.
    let w = watching_box();
    let other = w.f.path.with_file_name("b.silico");
    std::fs::write(&other, BOX).unwrap();
    let _second = w.f.sessions.open(&other, false).wait().session.unwrap().id;
    let (reload_started, through) = shut(&w.f.session().reloading);
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare_with_shared_reader();
    w.compare_with_shared_reader();
    reload_started.recv_timeout(WAIT).unwrap();
    std::fs::remove_file(&w.f.path).unwrap();
    std::os::unix::fs::symlink(&other, &w.f.path).unwrap();
    open_gate(&w.f.session().reloading);
    through.send(()).unwrap();
    w.reload_ends();
    assert_eq!(w.f.at(), Revision::OPENED);
    assert!(lock(&w.f.session().reloads).refused.is_none());
    w.settled_put_back_is_reloaded();
}

#[test]
fn a_settled_reload_inside_a_transaction_finding_another_sessions_file_claims_and_keeps_nothing() {
    // Queued behind a `begin`, the reload runs inside its transaction, and finds another
    // session's file linked at the path: neither the bytes the watch found settled nor the
    // session's, so it's neither claimed nor kept.
    let w = watching_box();
    let other = w.f.path.with_file_name("b.silico");
    std::fs::write(&other, BOX.replace("Block", "Brick")).unwrap();
    let _second = w.f.sessions.open(&other, false).wait().session.unwrap().id;
    let (reload_started, reload_through) = shut(&w.f.session().reloading);
    let (started, through) = shut(&w.f.session().gate);
    let begin = w.f.sessions.begin(&w.f.id);
    started.recv_timeout(WAIT).unwrap();
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare_with_shared_reader();
    w.compare_with_shared_reader();
    assert_eq!(w.told_count(), 1);
    assert!(lock(&w.f.session().reloads).queued);
    open_gate(&w.f.session().gate);
    through.send(()).unwrap();
    // The begin reloads the sphere itself (revision 2), then opens its transaction.
    assert_eq!(begin.wait().result.unwrap().base, Revision(2));
    reload_started.recv_timeout(WAIT).unwrap();
    std::fs::remove_file(&w.f.path).unwrap();
    std::os::unix::fs::symlink(&other, &w.f.path).unwrap();
    open_gate(&w.f.session().reloading);
    reload_through.send(()).unwrap();
    w.reload_ends();
    assert!(lock(&w.f.session().reloads).refused.is_none());
    assert_eq!(w.f.at(), Revision(2));
    assert!(w.f.status().transaction.is_some());
}

#[test]
fn a_settled_reload_queued_behind_a_begin_finds_the_file_changed_again_and_leaves_it_changed() {
    let w = watching_box();
    let (reload_started, reload_through) = shut(&w.f.session().reloading);
    let (started, through) = shut(&w.f.session().gate);
    let begin = w.f.sessions.begin(&w.f.id);
    started.recv_timeout(WAIT).unwrap();
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.compare();
    assert_eq!(w.told_count(), 1);
    assert!(lock(&w.f.session().reloads).queued);
    open_gate(&w.f.session().gate);
    through.send(()).unwrap();
    assert_eq!(begin.wait().result.unwrap().base, Revision(2));
    reload_started.recv_timeout(WAIT).unwrap();
    std::fs::write(&w.f.path, BOX.replace("Block", "Other")).unwrap();
    w.compare();
    assert_eq!(w.f.status().disk, Some(Disk::Changed));
    open_gate(&w.f.session().reloading);
    reload_through.send(()).unwrap();
    w.reload_ends();
    w.holds(Revision(2), 1, Disk::Changed);
}

#[test]
fn a_settled_reload_inside_a_transaction_finding_the_settled_bytes_leaves_the_finding_changed() {
    // Queued behind a `begin` that reloads other bytes, the reload runs inside its
    // transaction and finds the settled bytes back: claimed, as `commit` claims what it
    // reads, but nothing reloaded and nothing kept, and the finding stays changed.
    let w = watching_box();
    let (reload_started, reload_through) = shut(&w.f.session().reloading);
    let (started, through) = shut(&w.f.session().gate);
    let begin = w.f.sessions.begin(&w.f.id);
    started.recv_timeout(WAIT).unwrap();
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    w.compare();
    assert_eq!(w.told_count(), 1);
    assert!(lock(&w.f.session().reloads).queued);
    let other = BOX.replace("Block", "Other");
    std::fs::write(&w.f.path, &other).unwrap();
    open_gate(&w.f.session().gate);
    through.send(()).unwrap();
    assert_eq!(begin.wait().result.unwrap().base, Revision(2));
    reload_started.recv_timeout(WAIT).unwrap();
    std::fs::write(&w.f.path, SPHERE).unwrap();
    w.compare();
    assert_eq!(w.f.status().disk, Some(Disk::Changed));
    open_gate(&w.f.session().reloading);
    reload_through.send(()).unwrap();
    w.reload_ends();
    w.holds(Revision(2), 1, Disk::Changed);
    assert!(lock(&w.f.session().reloads).refused.is_none());
    // Read after the reload has ended: queued behind it, it would wait at its gate.
    assert_eq!(w.text_of(Revision(2)), other);
}
