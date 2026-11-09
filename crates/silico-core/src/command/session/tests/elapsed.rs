//! `status.rebuild.elapsed_ms` (Fable's decision of 2026-11-08, for the maintainer): the
//! whole milliseconds the revision's newest rebuild has run, from being asked for to its end
//! (its model published, or its failure or stop), so far while it runs; `null` with `done`
//! and `steps`. A clock's reading can't be pinned, so these pin what can be: when it's null,
//! that it never goes back, that it's fixed once the rebuild ends, and that it lies between
//! bounds the test's own monotonic clock (`Instant`) measures around the rebuild. Rebuilds
//! are held by the fake kernel's holds, and time is let pass by spinning on the test's clock,
//! never by a sleep.

use std::time::Instant;

use super::*;
use crate::command::wire::{Layout, ToWire, write};

/// What `status` writes of the session's rebuild: its state, and its `done`, `steps`, and
/// `elapsed_ms`, each `None` for `null`.
#[derive(Debug, PartialEq, Eq)]
struct Said {
    state: String,
    done: Option<u64>,
    steps: Option<u64>,
    elapsed_ms: Option<u64>,
}

/// The session `session`'s rebuild as `status` writes it, read back from its text.
fn said_of(f: &Fixture, session: &SessionId) -> Said {
    let status = f.sessions.status(session).result.unwrap();
    let text = write(&status.to_wire(), Layout::OneLine).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rebuild = &value["rebuild"];
    let whole = |key: &str| {
        let field = &rebuild[key];
        assert!(field.is_null() || field.is_u64(), "{key}: {text}");
        field.as_u64()
    };
    Said {
        state: rebuild["state"].as_str().unwrap().to_owned(),
        done: whole("done"),
        steps: whole("steps"),
        elapsed_ms: whole("elapsed_ms"),
    }
}

/// s1's rebuild, as `status` writes it ([`said_of`]).
fn said(f: &Fixture) -> Said {
    said_of(f, &f.id)
}

/// Spins until the test's own clock shows `gap` since `from`: time let pass, never slept.
fn spin_past(from: Instant, gap: Duration) {
    while from.elapsed() < gap {
        std::thread::yield_now();
    }
}

/// `gap` in whole milliseconds, floored, as `elapsed_ms` is written.
fn ms(gap: Duration) -> u64 {
    u64::try_from(gap.as_millis()).unwrap()
}

/// Waits until the session's newest rebuild's count has ended: told by the count, never
/// polled.
fn until_ended(f: &Fixture) {
    let counter = f.session().newest.counter().unwrap();
    let mut seen = 0;
    loop {
        let (count, changes) = counter.changed_since(seen);
        if count.ended {
            return;
        }
        seen = changes;
    }
}

/// Two readings of s1's rebuild at least 2 ms apart by the test's clock: equal only if its
/// clock is fixed, since a clock still running reads at least 2 more whole milliseconds the
/// second time (it read before the first answered, and after the gap).
fn two_readings(f: &Fixture) -> (Said, Said) {
    let first = said(f);
    let answered = Instant::now();
    spin_past(answered, Duration::from_millis(2));
    (first, said(f))
}

#[test]
fn null_until_a_rebuild_of_the_revision_starts_then_a_whole_number_with_done_and_steps() {
    // A box, opened, nothing has needed its model: `done`, `steps`, and `elapsed_ms` all
    // null. After a check, its one step done, and a time.
    let f = opened(BOX);
    assert_eq!(
        said(&f),
        Said {
            state: "none".to_owned(),
            done: None,
            steps: None,
            elapsed_ms: None,
        }
    );
    f.sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    let done = said(&f);
    assert_eq!(
        (done.state.as_str(), done.done, done.steps),
        ("done", Some(1), Some(1))
    );
    assert!(done.elapsed_ms.is_some(), "{done:?}");

    // A document the gate refuses (a box 0 long): nothing to rebuild, before a check and
    // after it, so all three stay null.
    let refused = opened(FLAT);
    let nothing = Said {
        state: "none".to_owned(),
        done: None,
        steps: None,
        elapsed_ms: None,
    };
    assert_eq!(said(&refused), nothing);
    let checked = refused
        .sessions
        .check(&refused.id, CheckModel::Current { fresh: false })
        .wait();
    assert!(!checked.result.unwrap().problems.is_empty());
    assert_eq!(said(&refused), nothing);
    assert_eq!(refused.made(is_box), 0);
}

#[test]
fn while_held_it_grows_with_the_tests_clock_and_once_done_it_stays() {
    // The box held: two readings 2 ms apart by the test's clock, both running, the second
    // at least 2 more (the first was read before it answered, the second after the gap).
    // Released: done, and fixed, however long after.
    let f = opened(BOX);
    let hold = f.kernel.hold(is_box);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(f.kernel.wait_running(1, WAIT));
    let (first, second) = two_readings(&f);
    assert_eq!(
        (first.state.as_str(), second.state.as_str()),
        ("running", "running")
    );
    assert_eq!((first.done, first.steps), (Some(0), Some(1)));
    let (first_ms, second_ms) = (first.elapsed_ms.unwrap(), second.elapsed_ms.unwrap());
    assert!(second_ms >= first_ms + 2, "{first_ms} then {second_ms}");
    hold.release();
    check.wait().result.unwrap();
    let (done, again) = two_readings(&f);
    assert_eq!(
        (done.state.as_str(), done.done, done.steps),
        ("done", Some(1), Some(1))
    );
    assert_eq!(done, again);
    // Its end came after the running readings: never back.
    assert!(
        done.elapsed_ms.unwrap() >= second_ms,
        "{done:?} after {second_ms}"
    );
    // And still, a later status later still.
    spin_past(Instant::now(), Duration::from_millis(2));
    assert_eq!(said(&f), done);
}

#[test]
fn it_lies_between_what_the_tests_clock_measures_around_the_rebuild() {
    // The test's clock read before the check is asked for (so before the rebuild's begin),
    // and again once the box is held (after its begin). Held until 50 ms have passed since
    // then: running, it reads at least 50, and at most the time since before the check.
    // Released: its end comes after the release, so at least 50 more than its begin; and
    // before the check answers, so at most the time since before the check.
    let f = opened(BOX);
    let hold = f.kernel.hold(is_box);
    let before = Instant::now();
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(f.kernel.wait_running(1, WAIT));
    let held = Instant::now();
    spin_past(held, Duration::from_millis(50));
    let running = said(&f).elapsed_ms.unwrap();
    let wall = ms(before.elapsed());
    assert!(
        (50..=wall).contains(&running),
        "{running} ms, not 50 to {wall}"
    );
    hold.release();
    check.wait().result.unwrap();
    let wall = ms(before.elapsed());
    let done = said(&f);
    assert_eq!(done.state, "done");
    let elapsed = done.elapsed_ms.unwrap();
    assert!(
        (running.max(50)..=wall).contains(&elapsed),
        "{elapsed} ms, not {running} to {wall}"
    );
}

#[test]
fn a_stopped_rebuild_reads_how_long_it_ran_fixed_once_its_thread_has_ended() {
    // The check's box held, the check cancelled: `stopped`, with a time. Once the stopped
    // rebuild's count has ended (its held box superseded, never released by the test), the
    // time is fixed.
    let f = opened(BOX);
    let _hold = f.kernel.hold(is_box);
    let check = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false });
    assert!(f.kernel.wait_running(1, WAIT));
    let started = said(&f).elapsed_ms.unwrap();
    assert!(
        f.sessions
            .cancel(&f.id, SessionCommand::Check)
            .result
            .unwrap()
            .stopped
    );
    assert_eq!(
        check.wait().result.unwrap_err().errors(),
        [cancelled("check")]
    );
    let stopped = said(&f);
    assert_eq!(stopped.state, "stopped");
    assert!(stopped.elapsed_ms.unwrap() >= started, "{stopped:?}");
    until_ended(&f);
    let (fixed, again) = two_readings(&f);
    assert_eq!(fixed.state, "stopped");
    assert_eq!(fixed, again);
    assert!(fixed.elapsed_ms.unwrap() >= stopped.elapsed_ms.unwrap());
    // The count's own end says the same.
    let counted = f.session().newest.counter().unwrap().now();
    assert!(counted.ended);
    assert_eq!(Some(ms(counted.elapsed)), fixed.elapsed_ms);
}

#[test]
fn a_faulted_rebuild_reads_how_long_it_ran_to_its_end_fixed() {
    // The box's worker crashes: `faulted`, its one step done, with a time, fixed.
    let f = opened(BOX);
    f.kernel
        .refuse(|op| is_box(op).then(|| Error::new(ErrorCode::WorkerCrashed, "crashed")));
    let _ = f
        .sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait();
    let (faulted, again) = two_readings(&f);
    assert_eq!(
        (faulted.state.as_str(), faulted.done, faulted.steps),
        ("faulted", Some(1), Some(1))
    );
    assert!(faulted.elapsed_ms.is_some(), "{faulted:?}");
    assert_eq!(faulted, again);
}

#[test]
fn a_rebuild_whose_every_step_is_cached_still_reads_a_time() {
    // The box rebuilt, the session closed and the file opened again: its new session's
    // rebuild takes its one step from the cache beside the file, the kernel asked for no
    // box. All done, and a time, however small: never null.
    let f = opened(BOX);
    f.sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    assert_eq!(f.made(is_box), 1);
    f.sessions.close(&f.id).wait().result.unwrap();
    let again = f.sessions.open(&f.path, false).wait().session.unwrap().id;
    f.sessions
        .check(&again, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    assert_eq!(f.made(is_box), 1, "the box was built again");
    let cached = said_of(&f, &again);
    assert_eq!(
        (cached.state.as_str(), cached.done, cached.steps),
        ("done", Some(1), Some(1))
    );
    assert!(cached.elapsed_ms.is_some(), "{cached:?}");
}

#[test]
fn a_new_revision_reads_null_until_a_read_rebuilds_it() {
    // Revision 1 rebuilt; then an edit in a transaction makes revision 2, which nothing has
    // read: null with `done` and `steps`, not revision 1's time. A check of it gives its own.
    let f = opened(BOX);
    f.sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    assert!(said(&f).elapsed_ms.is_some());
    f.begin();
    f.edit(&BOX.replace("\"length\": 40", "\"length\": 41"));
    assert_eq!(
        said(&f),
        Said {
            state: "none".to_owned(),
            done: None,
            steps: None,
            elapsed_ms: None,
        }
    );
    // 50 ms pass by the test's own clock after the first rebuild ended: a start kept per
    // session or per engine, not per rebuild, would make the second read at least 50.
    spin_past(Instant::now(), Duration::from_millis(50));
    let before = Instant::now();
    f.sessions
        .check(&f.id, CheckModel::Current { fresh: false })
        .wait()
        .result
        .unwrap();
    let wall = ms(before.elapsed());
    let rebuilt = said(&f);
    assert_eq!(
        (rebuilt.state.as_str(), rebuilt.done, rebuilt.steps),
        ("done", Some(1), Some(1))
    );
    let elapsed = rebuilt.elapsed_ms.unwrap();
    assert!(
        elapsed <= wall,
        "{elapsed} ms, more than the {wall} ms since just before its check"
    );
}

#[test]
fn elapsed_ms_is_the_whole_milliseconds_floored_and_saturates() {
    // `Progress::elapsed_ms`, which `elapsed_ms` writes and clients read: floored, never
    // rounded; past `u64`'s milliseconds, its last.
    let whole = |elapsed| {
        crate::command::Progress {
            done: 0,
            steps: 0,
            running: None,
            features: crate::command::FeatureStates::default(),
            elapsed,
        }
        .elapsed_ms()
    };
    assert_eq!(whole(Duration::ZERO), 0);
    assert_eq!(whole(Duration::from_micros(999)), 0);
    assert_eq!(whole(Duration::from_micros(84_999)), 84);
    assert_eq!(whole(Duration::from_secs(3)), 3000);
    assert_eq!(whole(Duration::MAX), u64::MAX);
}
