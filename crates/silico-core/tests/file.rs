//! M2.9: a document's file saved safely (`silico_core::file`), the cases a client meets
//! without timing: a file changed since it was read is left as it is, and the error says
//! to read it again.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::ErrorCode;

#[test]
fn a_save_over_a_file_changed_since_it_was_read_is_refused_and_says_to_read_it_again() {
    let dir = std::env::temp_dir().join(format!("silico-save-changed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("part.silico");
    std::fs::write(&file, b"edited").unwrap();
    let error = silico_core::file::save(&file, b"as read", b"formatted").unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoFailed);
    assert!(error.to_string().contains("read it again"), "{error}");
    assert_eq!(std::fs::read(&file).unwrap(), b"edited");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A directory of its own for one test, removed after.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("silico-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_document_of_exactly_the_most_a_reader_takes_is_read_whole_and_one_more_byte_shows() {
    // `read` reads up to one byte past the most a document may be, so the reader sees a
    // document of exactly the most whole, and one byte longer as too long (found by the
    // mutation run: reading one byte fewer passed every test).
    let most = silico_core::document::json::MAX_INPUT_LEN;
    let dir = Scratch::new("most");
    let file = dir.0.join("part.silico");
    let f = std::fs::File::create(&file).unwrap();
    f.set_len(most as u64).unwrap();
    assert_eq!(silico_core::file::read(&file).unwrap().len(), most);
    f.set_len(most as u64 + 10).unwrap();
    assert_eq!(silico_core::file::read(&file).unwrap().len(), most + 1);
}

#[test]
fn a_save_takes_the_next_name_beside_the_file_when_one_is_left_from_a_killed_run() {
    // A run killed with this process's number left `.part.silico.fmt-PID`: the save uses
    // `-1`, and leaves the other as it was (found by the mutation run: untested).
    let dir = Scratch::new("left");
    let file = dir.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    let left = format!(".part.silico.fmt-{}", std::process::id());
    std::fs::write(dir.0.join(&left), b"left").unwrap();
    silico_core::file::save(&file, b"as read", b"formatted").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"formatted");
    assert_eq!(dir.names(), [left.clone(), "part.silico".to_owned()]);
    assert_eq!(std::fs::read(dir.0.join(&left)).unwrap(), b"left");
}

#[test]
fn a_save_with_every_name_beside_the_file_taken_says_so_and_writes_nothing() {
    let dir = Scratch::new("taken");
    let file = dir.0.join("part.silico");
    std::fs::write(&file, b"as read").unwrap();
    let pid = std::process::id();
    std::fs::write(dir.0.join(format!(".part.silico.fmt-{pid}")), b"").unwrap();
    for n in 1..64 {
        std::fs::write(dir.0.join(format!(".part.silico.fmt-{pid}-{n}")), b"").unwrap();
    }
    let error = silico_core::file::save(&file, b"as read", b"formatted").unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoFailed);
    // All 64 names tried, and the message names them: the first, and -1 to -63.
    let said = format!(
        "every name for a file beside it is taken ({}/.part.silico.fmt-{pid}, and -1 to -63)",
        dir.0.display()
    );
    assert!(error.to_string().contains(&said), "{error}");
    assert_eq!(std::fs::read(&file).unwrap(), b"as read");
    assert_eq!(dir.names().len(), 65);
    // With the last name free, it's used: every name is tried.
    std::fs::remove_file(dir.0.join(format!(".part.silico.fmt-{pid}-63"))).unwrap();
    silico_core::file::save(&file, b"as read", b"formatted").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"formatted");
}
