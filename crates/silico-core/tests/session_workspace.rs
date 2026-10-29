//! Sessions in a workspace (docs/design/m4.1-command-api.md, decision 26 and section 7's
//! `open`; docs/design/m4.9-mcp.md, *What's added*): `open`'s path and `render`'s `to`
//! relative to the workspace's root, and a path outside it, or one that reaches out of it
//! through a link, refused as `invalid_input` naming the field, with the design's message and
//! suggestion; the check made again where a session's file is read again and saved, and
//! where its `.silico-cache/` is opened; and, with no workspace (the CLI), paths as the
//! system resolves them. Expected values are the design's text, the files' bytes as the
//! tests wrote them, and paths worked out by hand.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::command::render::Render;
use silico_core::command::{CheckModel, ModelChoice, Revision, SessionId, Sessions};
use silico_core::document::json::Node;
use silico_core::fake::{FakeKernel, FakeRenderer};
use silico_core::file::{Unresolved, Workspace};
use silico_core::kernel::{Op, Primitive};
use silico_core::views::NamedView;

use crate::calls::{BOX, Fixture, OUTSIDE, fixture, sessions_on};
use crate::sessions::{HANG, answer, id, refusal};

impl Fixture {
    /// The session `path` opened as, which must open.
    fn session_of(&self, path: impl Into<PathBuf>) -> SessionId {
        let response = answer(self.sessions.open(path, false));
        assert!(response.result.is_ok(), "{response:?}");
        response.session.unwrap().id
    }
}

/// The design's refusal of `given`, a path outside the workspace, in the request's `field`
/// (M4.1 section 7's `open` example, the path quoted as every message quotes one).
fn outside(given: &str, field: &str) -> (ErrorCode, String, Option<String>, Option<String>) {
    (
        ErrorCode::InvalidInput,
        format!("\"{given}\" is outside the workspace this server was started in"),
        Some(field.to_owned()),
        Some("give a path inside the workspace; a link out of it is refused too".to_owned()),
    )
}

/// The refusal of a command whose session's file, opened as `given`, now leads out of the
/// workspace.
fn outside_now(
    given: &str,
) -> (
    ErrorCode,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    (
        ErrorCode::InvalidInput,
        format!("\"{given}\" is outside the workspace this server was started in"),
        Some("path".to_owned()),
        Some(
            "it was inside the workspace when it was opened, and leads out of it now, through a link put on its path"
                .to_owned(),
        ),
        Some(
            "put the document back inside the workspace, or close the session and open one inside it"
                .to_owned(),
        ),
    )
}

/// The one error of a refused response: its code, message, parameter, cause, and suggestion.
fn refused_with_cause<T: std::fmt::Debug>(
    response: &silico_core::command::Response<T>,
) -> (
    ErrorCode,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let e = &problems.errors()[0];
    (
        e.code(),
        e.message().to_owned(),
        e.parameter().map(str::to_owned),
        e.cause().map(str::to_owned),
        e.suggestion().map(str::to_owned),
    )
}

fn value(text: &str) -> Node {
    silico_core::document::json::read(text.as_bytes()).unwrap()
}

/// `path` replaced by a link to `to`.
fn swap_for_link(path: &Path, to: &Path) {
    std::fs::remove_file(path).unwrap();
    std::os::unix::fs::symlink(to, path).unwrap();
}

#[test]
fn a_relative_path_is_opened_made_and_saved_in_the_workspace_and_named_as_given() {
    let f = fixture();
    f.file("part.silico", BOX);
    let response = answer(f.sessions.open("part.silico", false));
    let opened = response.result.as_ref().unwrap();
    assert_eq!(opened.path, "part.silico");
    let session = response.session.unwrap();
    assert_eq!(session.path, "part.silico");
    // An edit outside a transaction saves the file in the workspace, not one in the working
    // directory.
    let set = answer(f.sessions.set_field(
        &session.id,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        value("41"),
    ));
    assert!(set.result.unwrap().commit.unwrap().written);
    let saved = std::fs::read_to_string(f.work.join("part.silico")).unwrap();
    assert!(saved.contains("\"length\": 41,"), "{saved}");
    // An absolute path inside it, and a directory below its root.
    f.file("sub/b.silico", BOX);
    f.session_of(f.work.join("sub/b.silico"));
    // A new document, made in the workspace, relative to its root.
    let made = answer(f.sessions.open("new.silico", true));
    assert!(made.result.unwrap().created);
    assert!(f.work.join("new.silico").is_file());
    // A link inside it, to a file inside it, is no way out.
    std::os::unix::fs::symlink(f.work.join("new.silico"), f.work.join("alias.silico")).unwrap();
    // The file it leads to is open already, by another name: refused as that, not as out.
    let alias = answer(f.sessions.open("alias.silico", false));
    assert_eq!(
        refusal(&alias).1,
        "\"alias.silico\" is already open in session s3"
    );
}

#[test]
fn a_path_out_through_dot_dot_is_refused_whether_or_not_anything_is_there() {
    let f = fixture();
    // A `..` is taken as the system takes it, after the directory before it: `sub` is there.
    std::fs::create_dir(f.work.join("sub")).unwrap();
    for given in [
        "../outside/part.silico",
        "../outside/missing.silico",
        "sub/../../outside/part.silico",
        "..",
    ] {
        let response = answer(f.sessions.open(given, false));
        assert_eq!(refusal(&response), outside(given, "path"), "{given}");
        assert_eq!(response.session, None, "{given}");
    }
    // Made: refused before anything is made.
    let response = answer(f.sessions.open("../outside/new.silico", true));
    assert_eq!(refusal(&response), outside("../outside/new.silico", "path"));
    assert!(!f.outside.join("new.silico").exists());
    // A file inside that isn't there is what it was: `io_failed`.
    let missing = answer(f.sessions.open("missing.silico", false));
    let (code, message, parameter, _) = refusal(&missing);
    assert_eq!(
        (code, parameter.as_deref()),
        (ErrorCode::IoFailed, Some("path"))
    );
    assert!(message.starts_with("couldn't be read: "), "{message}");
    assert_eq!(f.theirs(), OUTSIDE);
}

#[test]
fn an_absolute_path_out_of_the_workspace_is_refused() {
    let f = fixture();
    let given = f.outside.join("part.silico");
    let response = answer(f.sessions.open(&given, false));
    assert_eq!(refusal(&response), outside(given.to_str().unwrap(), "path"));
    // The root's parent, and a sibling whose name starts with the root's.
    let sibling = f.work.with_file_name("work2");
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(sibling.join("part.silico"), BOX).unwrap();
    let given = sibling.join("part.silico");
    let response = answer(f.sessions.open(&given, true));
    assert_eq!(refusal(&response), outside(given.to_str().unwrap(), "path"));
    assert_eq!(std::fs::read_to_string(&given).unwrap(), BOX);
}

#[test]
fn a_link_out_of_the_workspace_to_a_file_or_a_directory_is_refused() {
    let f = fixture();
    std::os::unix::fs::symlink(f.outside.join("part.silico"), f.work.join("link.silico")).unwrap();
    std::os::unix::fs::symlink(&f.outside, f.work.join("out")).unwrap();
    // A link to nothing, outside: refused as out, not as a file missing.
    std::os::unix::fs::symlink(f.outside.join("none.silico"), f.work.join("gone.silico")).unwrap();
    for given in [
        "link.silico",
        "out/part.silico",
        "out/missing.silico",
        "gone.silico",
    ] {
        let response = answer(f.sessions.open(given, false));
        assert_eq!(refusal(&response), outside(given, "path"), "{given}");
    }
    // Made through a link out: nothing made.
    let response = answer(f.sessions.open("out/new.silico", true));
    assert_eq!(refusal(&response), outside("out/new.silico", "path"));
    assert!(!f.outside.join("new.silico").exists());
    assert_eq!(f.theirs(), OUTSIDE);
}

#[test]
fn a_file_swapped_for_a_link_out_after_open_is_refused_as_it_is_read_again() {
    let f = fixture();
    let path = f.file("part.silico", BOX);
    let s = f.session_of("part.silico");
    swap_for_link(&path, &f.outside.join("part.silico"));
    // Each queued command reads the file again first (decision 2): refused, not reloaded.
    for response in [
        refused_with_cause(&answer(f.sessions.text(&s, None))),
        refused_with_cause(&answer(
            f.sessions.check(&s, CheckModel::Current { fresh: false }),
        )),
    ] {
        assert_eq!(response, outside_now("part.silico"));
    }
    assert_eq!(
        f.sessions.status(&s).session.unwrap().revision,
        Revision::OPENED
    );
    // The file put back, it's read again, as it was.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, BOX).unwrap();
    let text = answer(f.sessions.text(&s, None));
    assert_eq!(text.result.unwrap().text, BOX);
    assert_eq!(f.theirs(), OUTSIDE);
}

#[test]
fn a_directory_swapped_for_a_link_out_after_open_is_refused_as_the_file_is_read_again() {
    let f = fixture();
    f.file("sub/part.silico", BOX);
    let s = f.session_of("sub/part.silico");
    std::fs::rename(f.work.join("sub"), f.work.join("moved")).unwrap();
    std::os::unix::fs::symlink(&f.outside, f.work.join("sub")).unwrap();
    let response = answer(f.sessions.text(&s, None));
    assert_eq!(
        refused_with_cause(&response),
        outside_now("sub/part.silico")
    );
}

#[test]
fn a_commit_after_its_file_was_swapped_for_a_link_out_is_refused_and_writes_nothing() {
    let f = fixture();
    let path = f.file("part.silico", BOX);
    let s = f.session_of("part.silico");
    answer(f.sessions.begin(&s)).result.unwrap();
    answer(f.sessions.set_field(
        &s,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        value("41"),
    ))
    .result
    .unwrap();
    swap_for_link(&path, &f.outside.join("part.silico"));
    let response = answer(f.sessions.commit(&s));
    assert_eq!(refused_with_cause(&response), outside_now("part.silico"));
    assert!(response.session.unwrap().transaction);
    assert_eq!(f.theirs(), OUTSIDE);
}

#[test]
fn a_save_whose_file_was_swapped_for_a_link_out_as_it_rebuilt_is_refused_and_writes_nothing() {
    let f = fixture();
    let path = f.file("part.silico", BOX);
    let s = f.session_of("part.silico");
    // The edit's commit read the file before the swap (inside then), and saves it after:
    // the save finds where the file leads now.
    let hold = f
        .kernel
        .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. })));
    let running = f.sessions.set_field(
        &s,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        value("41"),
    );
    assert!(f.kernel.wait_running(1, HANG));
    // To a file with the bytes the session read, so only where it leads is wrong.
    std::fs::write(f.outside.join("part.silico"), BOX).unwrap();
    swap_for_link(&path, &f.outside.join("part.silico"));
    hold.release();
    let response = answer(running);
    assert_eq!(refused_with_cause(&response), outside_now("part.silico"));
    assert_eq!(response.session.unwrap().revision, Revision::OPENED);
    assert_eq!(
        std::fs::read_to_string(f.outside.join("part.silico")).unwrap(),
        BOX
    );
}

#[test]
fn a_save_through_a_link_swapped_in_is_refused_by_the_file_layer_itself() {
    // `file::save_within` checks the file it would write, where it's found: a file swapped for
    // a link out since its session read it.
    let f = fixture();
    let path = f.file("part.silico", BOX);
    std::fs::write(f.outside.join("part.silico"), BOX).unwrap();
    let workspace = Workspace::new(&f.work).unwrap();
    let within = silico_core::file::Within {
        workspace: &workspace,
        given: Path::new("part.silico"),
    };
    silico_core::file::save_within(&path, BOX.as_bytes(), b"{}", Some(within)).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    swap_for_link(&path, &f.outside.join("part.silico"));
    let e = silico_core::file::save_within(&path, BOX.as_bytes(), b"{}", Some(within))
        .unwrap_err()
        .error;
    assert_eq!(
        (
            e.code(),
            e.message().to_owned(),
            e.parameter().map(str::to_owned),
            e.cause().map(str::to_owned),
            e.suggestion().map(str::to_owned),
        ),
        outside_now("part.silico")
    );
    assert_eq!(
        std::fs::read_to_string(f.outside.join("part.silico")).unwrap(),
        BOX
    );
    // With no workspace (the CLI's `save_over`), the link is saved through, as it always was.
    silico_core::file::save_over(&path, BOX.as_bytes(), b"{}").unwrap();
    assert_eq!(
        std::fs::read_to_string(f.outside.join("part.silico")).unwrap(),
        "{}"
    );
}

#[test]
fn a_cache_directory_that_leads_out_of_the_workspace_is_never_opened() {
    let f = fixture();
    f.file("part.silico", BOX);
    f.file("sub/part.silico", BOX);
    let theirs = f.outside.join("cache");
    std::fs::create_dir(&theirs).unwrap();
    std::os::unix::fs::symlink(&theirs, f.work.join(".silico-cache")).unwrap();
    let linked = f.session_of("part.silico");
    let plain = f.session_of("sub/part.silico");
    // Both build; only the one inside opens its cache.
    for s in [linked, plain] {
        let check = answer(f.sessions.check(&s, CheckModel::Current { fresh: false }));
        assert!(check.result.unwrap().problems.is_empty());
    }
    assert_eq!(
        *f.caches.lock().unwrap(),
        [f.work.join("sub/.silico-cache")]
    );
    assert_eq!(std::fs::read_dir(&theirs).unwrap().count(), 0);
    // A link to a cache directory inside the workspace is no way out: opened.
    std::fs::create_dir(f.work.join("kept")).unwrap();
    std::fs::remove_file(f.work.join(".silico-cache")).unwrap();
    f.file("third/part.silico", BOX);
    std::os::unix::fs::symlink(f.work.join("kept"), f.work.join("third/.silico-cache")).unwrap();
    f.session_of("third/part.silico");
    assert_eq!(
        f.caches.lock().unwrap().last(),
        Some(&f.work.join("third/.silico-cache"))
    );
}

fn view(to: &str) -> Render {
    Render {
        view: Some(NamedView::Front),
        camera: None,
        width: 40,
        height: 30,
        highlight: vec![],
        to: Some(PathBuf::from(to)),
        replace: false,
    }
}

#[test]
fn a_renders_to_is_relative_to_the_workspace_and_refused_outside_it() {
    let Fixture {
        _dir,
        work,
        outside: out,
        sessions,
        ..
    } = fixture();
    let renderer = Arc::new(FakeRenderer::new());
    let sessions = sessions.rendering(Arc::clone(&renderer));
    std::fs::write(work.join("part.silico"), BOX).unwrap();
    let s = answer(sessions.open("part.silico", false))
        .session
        .unwrap()
        .id;
    let done = answer(sessions.render(&s, ModelChoice::Current, view("view.png")));
    let rendering = done.result.unwrap();
    // Answered as the request gave it, relative to the workspace.
    assert_eq!(rendering.result.path, Some(PathBuf::from("view.png")));
    assert_eq!(std::fs::read(work.join("view.png")).unwrap(), b"PNG 40x30");
    std::os::unix::fs::symlink(&out, work.join("out")).unwrap();
    for given in [
        "../outside/view.png",
        "out/view.png",
        "../outside/none/view.png",
    ] {
        let response = answer(sessions.render(&s, ModelChoice::Current, view(given)));
        assert_eq!(refusal(&response), outside(given, "to"), "{given}");
    }
    let given = out.join("view.png");
    let response = answer(sessions.render(&s, ModelChoice::Current, view(given.to_str().unwrap())));
    assert_eq!(refusal(&response), outside(given.to_str().unwrap(), "to"));
    // Refused before anything was drawn.
    assert_eq!(renderer.sent().len(), 1);
    assert!(!out.join("view.png").exists());
}

#[test]
fn an_exports_path_is_relative_to_the_workspace_and_refused_outside_it() {
    // As a render's `to`: written inside the workspace, answered as given; refused outside
    // it, or through a link out of it, before anything is meshed.
    use silico_core::command::export::{Export, ExportFormat};
    let Fixture {
        _dir,
        work,
        outside: out,
        kernel,
        sessions,
        ..
    } = fixture();
    kernel.answer_instead(|op| match op {
        Op::ExportMesh { .. } => Some(silico_protocol::Reply::ExportMesh(Box::new(
            crate::export_report::tetra(),
        ))),
        _ => None,
    });
    std::fs::write(work.join("part.silico"), BOX).unwrap();
    let s = answer(sessions.open("part.silico", false))
        .session
        .unwrap()
        .id;
    let export = |path: &str| Export {
        format: ExportFormat::ThreeMf {
            tolerance: silico_core::document::Length::from_nm(100_000),
        },
        path: PathBuf::from(path),
        replace: false,
        features: None,
    };
    let done = answer(sessions.export(&s, export("part.3mf")))
        .result
        .unwrap();
    assert_eq!(done.path, PathBuf::from("part.3mf"));
    assert!(work.join("part.3mf").is_file());
    let meshed = || {
        kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::ExportMesh { .. }))
            .count()
    };
    assert_eq!(meshed(), 1);
    std::os::unix::fs::symlink(&out, work.join("out")).unwrap();
    for given in ["../outside/part.3mf", "out/part.3mf"] {
        let response = answer(sessions.export(&s, export(given)));
        assert_eq!(refusal(&response), outside(given, "path"), "{given}");
    }
    assert_eq!(meshed(), 1);
    assert!(!out.join("part.3mf").exists());
}

/// `path`, a path through its links, as a path relative to the working directory.
fn relative_to_working_directory(path: &Path) -> PathBuf {
    let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    let mut relative = PathBuf::new();
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    relative.join(path.strip_prefix("/").unwrap())
}

#[test]
fn without_a_workspace_paths_resolve_as_the_system_resolves_them() {
    // The CLI's sessions: no workspace, so a relative path is the working directory's, and
    // a path anywhere, through any link, opens, and its cache directory, a link or not, is
    // opened.
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (work, elsewhere) = (root.join("work"), root.join("elsewhere"));
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("part.silico"), BOX).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("part.silico"), work.join("link.silico")).unwrap();
    std::fs::create_dir(elsewhere.join("cache")).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("cache"), work.join(".silico-cache")).unwrap();
    std::fs::write(work.join("own.silico"), BOX).unwrap();
    let (_kernel, sessions, caches) = sessions_on(&root, |_| {});
    assert!(sessions.workspace().is_none());
    let relative = relative_to_working_directory(&work.join("own.silico"));
    let given = relative.to_str().unwrap().to_owned();
    let response = answer(sessions.open(&relative, false));
    assert_eq!(response.result.unwrap().path, given);
    let response = answer(sessions.open(work.join("link.silico"), false));
    let s = response.session.unwrap().id;
    assert!(
        answer(sessions.check(&s, CheckModel::Current { fresh: false }))
            .result
            .is_ok()
    );
    assert_eq!(
        *caches.lock().unwrap(),
        [work.join(".silico-cache"), elsewhere.join(".silico-cache")]
    );
    // And the file layer's resolution is the system's.
    assert_eq!(
        silico_core::file::resolve(None, &work.join("link.silico")).unwrap(),
        elsewhere.join("part.silico")
    );
    assert!(matches!(
        silico_core::file::resolve(None, &work.join("missing.silico")),
        Err(Unresolved::Unreachable(_))
    ));
}

#[test]
fn a_workspace_is_a_directory_given_once_before_any_document_opens() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("file"), "").unwrap();
    let code = |result: Result<Sessions<Arc<FakeKernel>>, silico_core::Error>| match result {
        Ok(_) => panic!("given a workspace"),
        Err(e) => (e.code(), e.message().to_owned()),
    };
    let (_, sessions, _) = sessions_on(&root, |_| {});
    assert_eq!(
        code(sessions.in_workspace(&root.join("file"))),
        (
            ErrorCode::InvalidInput,
            format!(
                "the workspace \"{}\" isn't a directory",
                root.join("file").display()
            )
        )
    );
    let (_, sessions, _) = sessions_on(&root, |_| {});
    let (missing, message) = code(sessions.in_workspace(&root.join("missing")));
    assert_eq!(missing, ErrorCode::IoFailed);
    assert!(message.starts_with("the workspace \""), "{message}");
    // Once only.
    let (_, sessions, _) = sessions_on(&root, |_| {});
    let sessions = sessions.in_workspace(&root).unwrap();
    assert_eq!(sessions.workspace().unwrap().root(), root);
    assert_eq!(
        code(sessions.in_workspace(&root)),
        (
            ErrorCode::InvalidInput,
            "these sessions can't be given a workspace now".to_owned()
        )
    );
    // Not once a document has been asked to open: those opened before would be outside its
    // rule.
    let (_, sessions, _) = sessions_on(&root, |_| {});
    let _ = answer(sessions.open(root.join("none.silico"), false));
    assert_eq!(
        code(sessions.in_workspace(&root)).1,
        "these sessions can't be given a workspace now"
    );
    // Its root through its links: a relative root, or one through a link, is held as the
    // directory it is.
    std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
    assert_eq!(Workspace::new(&root.join("link")).unwrap().root(), root);
}

/// `cache_clear` called by name on session `s`, which must run: its result, as a value.
fn cleared_by_name(f: &Fixture, s: &str) -> serde_json::Value {
    let cleared = f.call("cache_clear", &format!(r#"{{"session": "{s}"}}"#));
    assert!(cleared.ok, "{}", cleared.text);
    crate::calls::envelope(&cleared)["result"].clone()
}

/// `cache_clear` called by name on session `s`, open on `path` at revision 1, refused for
/// a cache that leads out of the workspace through `link` (from the workspace's root): the
/// refusal, as the envelope carries it (written out by hand).
fn refused_by_name(f: &Fixture, s: &str, path: &str, link: &str) {
    let cleared = f.call("cache_clear", &format!(r#"{{"session": "{s}"}}"#));
    assert!(!cleared.ok, "{}", cleared.text);
    assert_eq!(
        cleared.text,
        format!(
            r#"{{"ok": false, "session": {{"id": "{s}", "path": "{path}", "revision": 1, "transaction": false, "reloaded": false}}, "model": null, "errors": [{{"code": "invalid_input", "message": "the rebuild cache beside \"{path}\" leads out of the workspace, through a link, so it isn't cleared", "feature": null, "elements": [], "elements_total": 0, "parameter": null, "cause": "\"{link}\" is a link that leads out of the workspace", "suggestion": "remove the link, or clear the cache from inside the workspace", "candidates": []}}], "total": 1}}"#
        )
    );
}

/// The entry-named files at `cache`: `ab/` and `cd/`, each holding one named by 62 hex
/// digits, as the cache names its entries.
fn plant_entries(cache: &Path) -> Vec<PathBuf> {
    ["ab", "cd"]
        .into_iter()
        .map(|sub| {
            std::fs::create_dir_all(cache.join(sub)).unwrap();
            let entry = cache.join(sub).join(crate::cache_control::NAME);
            std::fs::write(&entry, b"theirs").unwrap();
            entry
        })
        .collect()
}

#[test]
fn cache_clear_through_a_cache_directory_linked_out_of_the_workspace_removes_nothing() {
    // `.silico-cache` beside the document is a link to a directory outside, holding files
    // named as entries are: the session's engine got a stand-in that keeps nothing, and a
    // clear removes none of them and is refused, saying why (never `removed: 0`, as an
    // empty cache's), the link named from the workspace's root, never where it leads.
    let f = fixture();
    f.file("part.silico", BOX);
    let theirs = f.outside.join("cache");
    let planted = plant_entries(&theirs);
    std::os::unix::fs::symlink(&theirs, f.work.join(".silico-cache")).unwrap();
    assert!(f.open("part.silico", false).ok);
    let check = f.call(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": true}"#,
    );
    assert!(check.ok, "{}", check.text);
    refused_by_name(&f, "s1", "part.silico", ".silico-cache");
    for entry in &planted {
        assert_eq!(std::fs::read(entry).unwrap(), b"theirs", "{entry:?}");
    }
    assert_eq!(crate::cache_control::entries(&theirs), planted);
    // Never opened, so nothing was written through it either.
    assert!(f.caches.lock().unwrap().is_empty());
}

#[test]
fn cache_clears_refusal_of_a_cache_linked_out_is_the_designs_example() {
    // The design's `bracket.silico`, its `.silico-cache` a link out: the refusal, written as
    // the error contract writes it, is the example's text byte for byte.
    use silico_core::command::wire::{Layout, ToWire, write};
    let f = fixture();
    f.file("bracket.silico", BOX);
    std::fs::create_dir(f.outside.join("cache")).unwrap();
    std::os::unix::fs::symlink(f.outside.join("cache"), f.work.join(".silico-cache")).unwrap();
    let s = answer(f.sessions.open("bracket.silico", false))
        .result
        .unwrap()
        .session;
    let problems = answer(f.sessions.cache_clear(&s)).result.unwrap_err();
    assert_eq!(problems.total(), 1);
    let text = write(&problems.errors()[0].to_wire(), Layout::Canonical).unwrap() + "\n";
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract/commands/examples/cache_clear/refused-leads-out.json");
    assert_eq!(text, std::fs::read_to_string(path).unwrap());
}

#[test]
fn cache_clear_after_its_directory_is_swapped_for_a_link_out_removes_nothing_through_it() {
    // A document in `sub/`, its cache opened inside the workspace and cleared there, named
    // from the workspace's root; then `sub` moved outside and a link to it put in its place:
    // the cache now leads out, and a clear removes nothing through it, refused, naming `sub`.
    let f = fixture();
    f.file("sub/part.silico", BOX);
    assert!(f.open("sub/part.silico", false).ok);
    let fresh = r#"{"session": "s1", "model": "current", "fresh": true}"#;
    assert!(f.call("check", fresh).ok);
    let cache = f.work.join("sub/.silico-cache");
    // One entry: the box, the document's one step.
    assert_eq!(crate::cache_control::entries(&cache).len(), 1);
    assert_eq!(
        cleared_by_name(&f, "s1"),
        serde_json::json!({"directory": "sub/.silico-cache", "removed": 1})
    );
    assert!(crate::cache_control::entries(&cache).is_empty());
    // Built anew, the entry kept again.
    assert!(f.call("check", fresh).ok);
    assert_eq!(crate::cache_control::entries(&cache).len(), 1);
    let moved = f.outside.join("sub");
    std::fs::rename(f.work.join("sub"), &moved).unwrap();
    std::os::unix::fs::symlink(&moved, f.work.join("sub")).unwrap();
    let kept = crate::cache_control::entries(&moved.join(".silico-cache"));
    assert_eq!(kept.len(), 1);
    refused_by_name(&f, "s1", "sub/part.silico", "sub");
    assert_eq!(
        crate::cache_control::entries(&moved.join(".silico-cache")),
        kept
    );
}

#[test]
fn cache_clear_of_a_stand_in_whose_documents_directory_is_swapped_out_removes_nothing() {
    let f = fixture();
    f.file("sub/part.silico", BOX);
    let theirs = f.outside.join("cache");
    std::fs::create_dir_all(&theirs).unwrap();
    let link = f.work.join("sub/.silico-cache");
    std::os::unix::fs::symlink(&theirs, &link).unwrap();
    assert!(f.open("sub/part.silico", false).ok);
    assert!(f.caches.lock().unwrap().is_empty());
    std::fs::remove_file(&link).unwrap();
    let moved = f.outside.join("sub");
    std::fs::rename(f.work.join("sub"), &moved).unwrap();
    std::os::unix::fs::symlink(&moved, f.work.join("sub")).unwrap();
    let planted = plant_entries(&moved.join(".silico-cache"));
    refused_by_name(&f, "s1", "sub/part.silico", "sub");
    for entry in &planted {
        assert_eq!(std::fs::read(entry).unwrap(), b"theirs", "{entry:?}");
    }
}

#[test]
fn cache_clear_of_a_cache_that_led_out_as_it_opened_and_is_inside_as_it_clears_clears_it() {
    // Whether a clear goes through is decided as it clears: `.silico-cache` a link out as the
    // session opens (its engine's cache a stand-in, writing nothing there), then the link
    // replaced by a directory inside the workspace holding two entries: the clear removes
    // them, as it would any cache inside the workspace, and what's outside is untouched.
    let f = fixture();
    f.file("part.silico", BOX);
    let theirs = f.outside.join("cache");
    let planted = plant_entries(&theirs);
    let link = f.work.join(".silico-cache");
    std::os::unix::fs::symlink(&theirs, &link).unwrap();
    assert!(f.open("part.silico", false).ok);
    // The stand-in: no cache opened.
    assert!(f.caches.lock().unwrap().is_empty());
    std::fs::remove_file(&link).unwrap();
    let inside = plant_entries(&link);
    assert_eq!(inside.len(), 2);
    assert_eq!(
        cleared_by_name(&f, "s1"),
        serde_json::json!({"directory": ".silico-cache", "removed": 2})
    );
    assert!(crate::cache_control::entries(&link).is_empty());
    assert_eq!(crate::cache_control::entries(&theirs), planted);
}
