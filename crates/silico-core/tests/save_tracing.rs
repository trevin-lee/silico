//! A save says in its span when it syncs the disk and how long that took, so a disk that
//! stalls a save shows in the log as the line it stopped at (CI's runs stalled in `fmt`'s
//! sync for over a minute, 2026-10-22 UTC, and logged nothing to say so).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// `bytes` saved over `file`, which held `before` ([`silico_core::file::save_over`]): why
/// not, if not.
fn save(file: &std::path::Path, before: &[u8], bytes: &[u8]) -> Result<(), silico_core::Error> {
    silico_core::file::save_over(file, before, bytes).map_err(|unsaved| unsaved.error)
}

/// An event: the spans it's in, nearest first, its message, its `path`, and its `sync_ms`.
#[derive(Debug, PartialEq)]
struct Said {
    spans: Vec<&'static str>,
    message: String,
    path: Option<String>,
    sync_ms: Option<u64>,
}

#[derive(Default)]
struct Seen {
    said: Vec<Said>,
    /// Each `save` span's `bytes`.
    saves: Vec<u64>,
}

struct Recorder(Arc<Mutex<Seen>>);

#[derive(Default)]
struct Fields {
    message: String,
    path: Option<String>,
    sync_ms: Option<u64>,
    bytes: Option<u64>,
}

impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "sync_ms" => self.sync_ms = Some(value),
            "bytes" => self.bytes = Some(value),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "path" => self.path = Some(format!("{value:?}")),
            _ => {}
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Recorder {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        if attrs.metadata().name() == "save" {
            let mut fields = Fields::default();
            attrs.record(&mut fields);
            self.0.lock().unwrap().saves.push(fields.bytes.unwrap());
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let spans = ctx
            .event_scope(event)
            .map(|scope| scope.map(|s| s.name()).collect())
            .unwrap_or_default();
        self.0.lock().unwrap().said.push(Said {
            spans,
            message: fields.message,
            path: fields.path,
            sync_ms: fields.sync_ms,
        });
    }
}

/// What `save` said, the `bytes` of each save span, and how long the save took.
fn saved(file: &std::path::Path, before: &[u8], bytes: &[u8]) -> (Seen, Duration) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let subscriber = tracing_subscriber::registry().with(Recorder(Arc::clone(&seen)));
    let start = Instant::now();
    tracing::subscriber::with_default(subscriber, || {
        save(file, before, bytes).unwrap();
    });
    let took = start.elapsed();
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    (seen, took)
}

/// The syncing and synced lines, in the order said, each of what's synced at its path, in
/// the save's span, the second with its time and the first without; and no time longer
/// than the whole save took.
fn syncs(seen: &Seen, took: Duration, whats: &[(&str, &std::path::Path)]) {
    let expected: Vec<(String, Option<String>, bool)> = whats
        .iter()
        .flat_map(|(what, path)| {
            let path = Some(path.display().to_string());
            [
                (format!("syncing {what}"), path.clone(), false),
                (format!("synced {what}"), path, true),
            ]
        })
        .collect();
    let said: Vec<(String, Option<String>, bool)> = seen
        .said
        .iter()
        .map(|s| (s.message.clone(), s.path.clone(), s.sync_ms.is_some()))
        .collect();
    assert_eq!(said, expected);
    for s in &seen.said {
        assert_eq!(s.spans, ["save"], "{s:?}");
        if let Some(ms) = s.sync_ms {
            assert!(
                u128::from(ms) <= took.as_millis(),
                "{ms} ms of a {took:?} save"
            );
        }
    }
}

/// A directory of its own for one test, removed after.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("silico-save-tracing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_save_says_when_it_syncs_its_new_text_and_how_long_that_took() {
    let scratch = Scratch::new("renamed");
    let file = scratch.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    let (seen, took) = saved(&file, b"as read", b"the formatted text");
    assert_eq!(seen.saves, [18]);
    // The new text beside it, `.NAME.fmt-PID`, then the directory it's renamed in, the
    // file's parent (the temporary directory's links resolved, as the save resolves the
    // file's), so the rename is on disk too: a power cut after the save can't bring back
    // the old file.
    let directory = std::fs::canonicalize(&scratch.0).unwrap();
    let beside = directory.join(format!(".part.silico.fmt-{}", std::process::id()));
    syncs(
        &seen,
        took,
        &[
            ("the new text beside it", &beside),
            ("the directory it was renamed in", &directory),
        ],
    );
}

#[test]
fn a_file_a_command_writes_syncs_its_new_file_and_then_the_directory_it_was_renamed_in() {
    // A render's PNG (`file::write_output`): the new file beside it, then the directory it's
    // renamed in, so a power cut after the write can't bring back the old file, as a save's.
    let scratch = Scratch::new("output");
    let directory = std::fs::canonicalize(&scratch.0).unwrap();
    let rules = silico_core::file::Output {
        field: "to",
        extensions: &["png"],
        writer: "a render",
        replace: false,
        workspace: Some(&silico_core::file::Workspace::new(&directory).unwrap()),
        held: &[],
    };
    let seen = Arc::new(Mutex::new(Seen::default()));
    let subscriber = tracing_subscriber::registry().with(Recorder(Arc::clone(&seen)));
    tracing::subscriber::with_default(subscriber, || {
        silico_core::file::write_output(&directory.join("view.png"), b"PNG", &rules).unwrap();
    });
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    let beside = directory.join(format!(".view.png.part-{}", std::process::id()));
    let said: Vec<(&str, Option<String>)> = seen
        .said
        .iter()
        .filter(|s| s.message.starts_with("sync"))
        .map(|s| (s.message.as_str(), s.path.clone()))
        .collect();
    let at = |p: &std::path::Path| Some(p.display().to_string());
    assert_eq!(
        said,
        [
            ("syncing the new file", at(&beside)),
            ("synced the new file", at(&beside)),
            ("syncing the directory it was renamed in", at(&directory)),
            ("synced the directory it was renamed in", at(&directory)),
        ]
    );
    assert_eq!(std::fs::read(directory.join("view.png")).unwrap(), b"PNG");
}

#[test]
fn a_save_in_place_says_when_it_syncs_the_old_text_kept_and_then_the_new() {
    let scratch = Scratch::new("linked");
    let file = scratch.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    std::fs::hard_link(&file, scratch.0.join("other-name.silico")).unwrap();
    let (seen, took) = saved(&file, b"as read", b"formatted");
    assert_eq!(seen.saves, [9]);
    let directory = std::fs::canonicalize(&scratch.0).unwrap();
    let kept = directory.join(format!(".part.silico.fmt-{}", std::process::id()));
    syncs(
        &seen,
        took,
        &[
            ("its old text kept beside it", &kept),
            ("its new text, in place", &directory.join("part.silico")),
        ],
    );
}

#[test]
fn a_save_refused_before_it_writes_syncs_nothing() {
    let scratch = Scratch::new("changed");
    let file = scratch.0.join("part.silico");
    std::fs::write(&file, b"edited").unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let subscriber = tracing_subscriber::registry().with(Recorder(Arc::clone(&seen)));
    tracing::subscriber::with_default(subscriber, || {
        save(&file, b"as read", b"formatted").unwrap_err();
    });
    let seen = seen.lock().unwrap();
    assert_eq!(seen.saves, [9]);
    assert_eq!(seen.said, []);
}
