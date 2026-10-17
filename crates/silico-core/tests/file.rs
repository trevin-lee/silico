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
