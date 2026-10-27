//! break-it: M4.3's cache control under hostile requests and disks. `fresh` as anything but a
//! boolean, and with the published model; a fresh check of a sketch whose failure the engine
//! kept, of a session dropped mid-rebuild, and twice running; `cache_clear` of a cache
//! directory that's a file, a link, unreadable, or full of things named like entries that
//! aren't (a pipe among them, which a read would hang on), during a rebuild of the same
//! session, and of a session whose file has gone. Expected values are by hand: the fake
//! kernel's and solver's call records, and counts of the entries planted.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use silico_core::ErrorCode;
use silico_core::command::wire::reads::CheckRequest;
use silico_core::command::{CacheCleared, CheckModel, Response, SessionId};
use silico_core::document::json::read;

use crate::cache_control::{Fixture, NAME, fixture};
use crate::example::EXAMPLE;
use crate::sessions::{HANG, answer};

impl Fixture {
    fn clear(&self, id: &SessionId) -> Response<CacheCleared> {
        answer(self.sessions.cache_clear(id))
    }
}

/// The codes and parameters a request's refusal gives.
fn refused(text: &str) -> Vec<(ErrorCode, Option<String>, String)> {
    CheckRequest::read(&read(text.as_bytes()).unwrap())
        .unwrap_err()
        .errors()
        .iter()
        .map(|e| {
            (
                e.code(),
                e.parameter().map(str::to_owned),
                e.message().to_owned(),
            )
        })
        .collect()
}

#[test]
fn fresh_must_be_a_boolean_and_only_true_with_the_current_model() {
    for (value, written) in [
        (r#""true""#, r#""true""#),
        ("1", "1"),
        ("0", "0"),
        ("null", "null"),
        ("[]", "an array"),
        ("{}", "an object"),
    ] {
        let text = format!(r#"{{"session": "s1", "model": "current", "fresh": {value}}}"#);
        assert_eq!(
            refused(&text),
            [(
                ErrorCode::InvalidInput,
                Some("fresh".to_owned()),
                format!("fresh must be true or false, got {written}")
            )],
            "{value}"
        );
    }
    // Missing, or given twice, refused as any field is.
    let missing = refused(r#"{"session": "s1", "model": "current"}"#);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].1.as_deref(), Some("fresh"));
    // With published, refused once, naming fresh, whatever else is wrong beside it.
    assert_eq!(
        refused(r#"{"session": "s1", "model": "published", "fresh": true}"#),
        [(
            ErrorCode::InvalidInput,
            Some("fresh".to_owned()),
            "fresh must be false for a read of the published model".to_owned()
        )]
    );
    let both = refused(r#"{"session": "s1", "model": "published", "fresh": true, "x": 1}"#);
    let parameters: Vec<Option<&str>> = both.iter().map(|(_, p, _)| p.as_deref()).collect();
    // The fields' reasons first, in their order, then the keys it doesn't take, as every
    // request's.
    assert_eq!(parameters, [Some("fresh"), Some("x")], "{both:?}");
    // A model that isn't one, with fresh true: the model's refusal alone, since which
    // model it would go with isn't known.
    let unknown = refused(r#"{"session": "s1", "model": "newest", "fresh": true}"#);
    let parameters: Vec<Option<&str>> = unknown.iter().map(|(_, p, _)| p.as_deref()).collect();
    assert_eq!(parameters, [Some("model")], "{unknown:?}");
}

#[test]
fn a_fresh_check_solves_again_a_sketch_whose_failure_the_engine_kept() {
    // Over-constrained, the solve's failure follows from the sketch, so the engine keeps
    // it: a plain check of the revision asks nothing; a fresh one asks the solver again.
    let f = fixture();
    f.sketches.answer(|_| {
        Err(silico_core::Error::new(
            ErrorCode::OverConstrained,
            "the sketch is over-constrained",
        ))
    });
    let sketch = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "A", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}]}"#;
    let a = f.open("a.silico", sketch);
    let first = answer(f.sessions.check(&a, CheckModel::Current { fresh: false }));
    let codes = |r: &Response<silico_core::command::CheckAnswer>| -> Vec<ErrorCode> {
        r.result
            .as_ref()
            .unwrap()
            .problems
            .errors()
            .iter()
            .map(|e| e.code())
            .collect()
    };
    assert_eq!(codes(&first), [ErrorCode::OverConstrained]);
    assert_eq!(f.sketches.requests().len(), 1);
    let again = answer(f.sessions.check(&a, CheckModel::Current { fresh: false }));
    assert_eq!(codes(&again), [ErrorCode::OverConstrained]);
    assert_eq!(f.sketches.requests().len(), 1);
    let fresh = answer(f.sessions.check(&a, CheckModel::Current { fresh: true }));
    assert_eq!(codes(&fresh), [ErrorCode::OverConstrained]);
    assert_eq!(f.sketches.requests().len(), 2);
    assert!(f.kernel.calls().is_empty());
}

#[test]
fn fresh_checks_one_after_another_each_build_every_step() {
    // Each is its own forced rebuild: three fresh checks, three boxes; never a hit of the
    // one before.
    let f = fixture();
    let block = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;
    let a = f.open("a.silico", block);
    let checks: Vec<_> = (0..3)
        .map(|_| f.sessions.check(&a, CheckModel::Current { fresh: true }))
        .collect();
    for check in checks {
        let checked = answer(check);
        assert!(checked.result.unwrap().problems.is_empty());
        assert_eq!(checked.model.unwrap().fault, None);
    }
    let boxes = f
        .kernel
        .calls()
        .iter()
        .filter(|call| call.op.name() == "MakeBox")
        .count();
    assert_eq!(boxes, 3);
}

#[test]
fn sessions_dropped_while_a_fresh_check_waits_answer_it_and_hang_nothing() {
    let f = fixture();
    let a = f.open("a.silico", EXAMPLE);
    let hold = f.kernel.hold(|op| op.name() == "Cut");
    let check = f.sessions.check(&a, CheckModel::Current { fresh: true });
    assert!(f.kernel.wait_running(1, HANG));
    let Fixture {
        dir: _dir,
        kernel,
        sessions,
        ..
    } = f;
    drop(sessions);
    let answered = check.wait_timeout(HANG).ok().expect("no answer");
    let refused = answered.result.unwrap_err();
    let error = &refused.errors()[0];
    assert!(
        matches!(error.code(), ErrorCode::InvalidInput | ErrorCode::Cancelled),
        "{error}"
    );
    drop(hold);
    assert!(kernel.wait_idle(HANG));
}

#[test]
fn a_cache_directory_thats_a_file_a_link_or_unreadable_is_cleared_safely() {
    let f = fixture();
    let a = f.open("a.silico", EXAMPLE);
    // A file where the directory goes: nothing removed, the file kept.
    std::fs::write(f.cache(), b"not a directory").unwrap();
    assert_eq!(f.clear(&a).result.unwrap().removed, 0);
    assert_eq!(std::fs::read(f.cache()).unwrap(), b"not a directory");
    std::fs::remove_file(f.cache()).unwrap();
    // A link to another cache's directory: what's there is never removed.
    let elsewhere = f.dir.path().join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("ab")).unwrap();
    let planted = elsewhere.join("ab").join(NAME);
    std::fs::write(&planted, b"someone's file").unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.cache()).unwrap();
    assert_eq!(f.clear(&a).result.unwrap().removed, 0);
    assert_eq!(std::fs::read(&planted).unwrap(), b"someone's file");
    std::fs::remove_file(f.cache()).unwrap();
    // Built, then made unreadable: refused, naming the directory, nothing removed.
    let checked = answer(f.sessions.check(&a, CheckModel::Current { fresh: false }));
    assert!(checked.result.unwrap().problems.is_empty());
    assert_eq!(f.entries().len(), 4);
    std::fs::set_permissions(f.cache(), std::fs::Permissions::from_mode(0o333)).unwrap();
    // Root reads it anyway, and the case would test nothing: said, never passed quietly.
    assert!(
        std::fs::read_dir(f.cache()).is_err(),
        "the cache's directory is still readable at 0o333 (run as root?): this case can't be tested"
    );
    {
        let refused = f.clear(&a).result.unwrap_err();
        assert_eq!(refused.total(), 1);
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::IoFailed);
        assert!(
            error.message().starts_with(&format!(
                "couldn't read the cache's directory \"{}\": ",
                f.cache().display()
            )),
            "{}",
            error.message()
        );
        assert_eq!(
            error.cause(),
            Some("the clear removed 0 entries and left what it couldn't remove or read")
        );
    }
    std::fs::set_permissions(f.cache(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(f.clear(&a).result.unwrap().removed, 4);
}

#[test]
fn a_pipe_or_a_socket_named_as_an_entry_is_left_and_never_waited_on() {
    let f = fixture();
    let a = f.open("a.silico", EXAMPLE);
    assert!(
        answer(f.sessions.check(&a, CheckModel::Current { fresh: false }))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let entries = f.entries();
    assert_eq!(entries.len(), 4);
    let sub = entries[0].parent().unwrap().to_owned();
    let pipe = sub.join(NAME);
    let made = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .unwrap();
    assert!(made.success());
    // Bound in place, through a link under /tmp to the subdirectory, so the path given fits
    // a socket's (104 bytes on macOS, 108 on Linux) wherever the cache is: the socket is
    // made in the subdirectory itself, never moved there (a rename between filesystems
    // fails, EXDEV).
    let short = tempfile::tempdir_in("/tmp").unwrap();
    let link = short.path().join("l");
    std::os::unix::fs::symlink(&sub, &link).unwrap();
    let socket_name = NAME.replace('0', "1");
    let _listening = std::os::unix::net::UnixListener::bind(link.join(&socket_name)).unwrap();
    let socket = sub.join(&socket_name);
    let cleared = f.clear(&a).result.unwrap();
    assert_eq!(cleared.removed, 4);
    let kind = |path: &Path| std::fs::symlink_metadata(path).unwrap().file_type();
    use std::os::unix::fs::FileTypeExt as _;
    assert!(kind(&pipe).is_fifo());
    assert!(kind(&socket).is_socket());
}

#[test]
fn a_clear_during_a_rebuild_of_the_same_session_leaves_only_sound_entries() {
    // A published read starts the edited revision's rebuild, held at its cut; the clear,
    // queued with nothing ahead of it, runs meanwhile. Whatever it removed is a miss, what
    // the rebuild writes after is whole: the rebuild's model is right, and a new session's
    // rebuild from what's left is right too.
    let f = fixture();
    let a = f.open("a.silico", EXAMPLE);
    assert!(
        answer(f.sessions.check(&a, CheckModel::Current { fresh: false }))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let edited = EXAMPLE.replace(r#""radius": 2.5"#, r#""radius": 3"#);
    assert_ne!(edited, EXAMPLE);
    let revision = f.sessions.status(&a).session.unwrap().revision;
    let hold = f.kernel.hold(|op| op.name() == "Cut");
    // In a transaction, so the edit is applied without a rebuild of its own to commit.
    assert!(answer(f.sessions.begin(&a)).result.is_ok());
    let edit = answer(f.sessions.replace_text(&a, revision, edited.clone()));
    assert!(edit.result.is_ok(), "{edit:?}");
    let started = answer(f.sessions.check(&a, CheckModel::Published));
    assert!(started.result.is_ok());
    assert!(f.kernel.wait_running(1, HANG));
    let cleared = f.clear(&a).result.unwrap();
    // The four the first check wrote, and what the edit's rebuild had written by then
    // (its pin and the pin's move): four to six.
    assert!((4..=6).contains(&cleared.removed), "{cleared:?}");
    drop(hold);
    let checked = answer(f.sessions.check(&a, CheckModel::Current { fresh: false }));
    assert!(checked.result.unwrap().problems.is_empty());
    assert!(answer(f.sessions.rollback(&a)).result.is_ok());
    // Every entry on disk now is whole: a session of its own on the edited document takes
    // what's there and builds the rest, the same shapes.
    let b = f.open("b.silico", &edited);
    let rebuilt = answer(f.sessions.check(&b, CheckModel::Current { fresh: false }));
    assert!(rebuilt.result.unwrap().problems.is_empty());
    assert_eq!(rebuilt.model.unwrap().fault, None);
}

#[test]
fn a_clear_of_a_session_whose_file_has_gone_or_changed_clears_and_reads_nothing() {
    // `cache_clear` needs nothing of the document, so it never reads the file (decision 2,
    // as `close`): gone, the clear runs all the same, and the session stays at its
    // revision, not reloaded.
    let f = fixture();
    let a = f.open("a.silico", EXAMPLE);
    assert!(
        answer(f.sessions.check(&a, CheckModel::Current { fresh: false }))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let revision = f.sessions.status(&a).session.unwrap().revision;
    std::fs::remove_file(f.docs().join("a.silico")).unwrap();
    let cleared = f.clear(&a);
    assert_eq!(cleared.result.unwrap().removed, 4);
    let session = cleared.session.unwrap();
    assert_eq!(session.revision, revision);
    assert!(!session.reloaded);
    assert!(f.entries().is_empty());
    assert_eq!(f.sessions.status(&a).session.unwrap().revision, revision);
    // Changed outside the session: not reloaded either.
    let b = f.open("b.silico", EXAMPLE);
    assert!(
        answer(f.sessions.check(&b, CheckModel::Current { fresh: false }))
            .result
            .unwrap()
            .problems
            .is_empty()
    );
    let revision = f.sessions.status(&b).session.unwrap().revision;
    let changed = EXAMPLE.replace(r#""radius": 2.5"#, r#""radius": 3"#);
    assert_ne!(changed, EXAMPLE);
    std::fs::write(f.docs().join("b.silico"), &changed).unwrap();
    let cleared = f.clear(&b);
    assert_eq!(cleared.result.unwrap().removed, 4);
    let session = cleared.session.unwrap();
    assert_eq!(session.revision, revision);
    assert!(!session.reloaded);
    assert_eq!(f.sessions.status(&b).session.unwrap().revision, revision);
}
