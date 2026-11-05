//! Hostile paths against a workspace (docs/design/m4.1-command-api.md, decision 26;
//! docs/design/m4.9-mcp.md, *What's added*): a root that's a link, removed after start, or
//! swapped for a link mid-session; FIFOs and devices where a document or a render's `to` is
//! looked for; directories, empty paths, `.`, `..` that leads back inside, NUL, very long
//! paths, case variants, link loops and chains, outside directories deeper than the links
//! the check follows; and a cache directory swapped for a link out mid-session. Expected
//! values: POSIX (what `open(2)` of the same path does), the design's rule ("a path out of
//! the workspace is refused as one, whether or not anything is there"), and the files'
//! bytes as the tests wrote them.

use crate::model::WAIT;
use std::path::{Path, PathBuf};
use std::time::Duration;

use silico_core::command::{Answered, Calling};

use crate::calls::{
    BOX, Fixture, OUTSIDE, REFUSAL_BOUND, call, calling, envelope, fixture, open_request,
    sessions_in,
};

/// The longest a call that refuses a FIFO or a device may take: it never waits on one.
const PROMPT: Duration = WAIT;

impl Fixture {
    /// A render of session `session`'s current model, written to `to`.
    fn render_to(&self, session: &str, to: &str) -> Answered {
        self.call(
            "render",
            &format!(
                r#"{{"session": "{session}", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": {}, "replace": false}}"#,
                serde_json::to_string(to).unwrap()
            ),
        )
    }
}

/// The call's answer, failing if it takes longer than [`PROMPT`].
fn promptly(calling: Calling, what: &str) -> Answered {
    match calling.wait_timeout(PROMPT) {
        Ok(answered) => answered,
        Err(_) => panic!("{what} didn't answer in {PROMPT:?}: it waits on a FIFO or a device"),
    }
}

/// The first error of a refused answer: its code, message, and parameter.
fn error(answered: &Answered) -> (String, String, Option<String>) {
    assert!(!answered.ok, "{}", answered.text);
    let e = &envelope(answered)["errors"][0];
    (
        e["code"].as_str().unwrap().to_owned(),
        e["message"].as_str().unwrap().to_owned(),
        e["parameter"].as_str().map(str::to_owned),
    )
}

fn is_outside(answered: &Answered) -> bool {
    !answered.ok && error(answered).1.contains("is outside the workspace")
}

fn mkfifo(path: &Path) {
    let made = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success());
}

#[test]
fn a_root_given_through_a_link_holds_its_files_by_every_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), BOX).unwrap();
    std::os::unix::fs::symlink(&work, root.join("link")).unwrap();
    let sessions = sessions_in(&root, &root.join("link"));
    let open = call(&sessions, "open", &open_request("part.silico", false));
    assert!(open.ok, "{}", open.text);
    // The same file through the link's spelling, and its own: inside, and held by s1.
    for spelling in [root.join("link/part.silico"), work.join("part.silico")] {
        let again = call(
            &sessions,
            "open",
            &open_request(spelling.to_str().unwrap(), false),
        );
        let (code, message, parameter) = error(&again);
        assert!(
            !message.contains("outside the workspace"),
            "{}: {message}",
            spelling.display()
        );
        assert_eq!(
            (code.as_str(), parameter.as_deref()),
            ("invalid_input", Some("path"))
        );
    }
    // A file made through the link's spelling is in the workspace too.
    let made = call(
        &sessions,
        "open",
        &open_request(root.join("link/new.silico").to_str().unwrap(), true),
    );
    assert!(made.ok, "{}", made.text);
    assert!(work.join("new.silico").is_file());
}

#[test]
fn a_root_removed_after_start_is_said_unreachable_not_outside() {
    // Broke (break-it): with the workspace's directory gone, a path inside it resolves to
    // nothing, and `Workspace::leads_out` walked up to the nearest directory that's there,
    // the root's parent, which is outside the root: so `part.silico` was refused as "outside
    // the workspace", which it isn't, and an agent told to give a path inside the workspace
    // couldn't. POSIX's answer for the same open is ENOENT: the directory can't be reached.
    let f = fixture();
    std::fs::remove_dir(&f.work).unwrap();
    for (path, create) in [
        ("part.silico", false),
        ("new.silico", true),
        ("sub/part.silico", false),
    ] {
        let answer = f.open(path, create);
        assert!(!answer.ok, "{}", answer.text);
        let (code, message, parameter) = error(&answer);
        assert_eq!(parameter.as_deref(), Some("path"), "{path}");
        assert!(
            !message.contains("outside the workspace"),
            "{path}: told {message:?}"
        );
        assert_eq!(code, "io_failed", "{path}: {message}");
    }
}

#[test]
fn a_fifo_or_a_device_is_refused_at_once_never_waited_on() {
    let f = fixture();
    mkfifo(&f.work.join("pipe.silico"));
    std::os::unix::fs::symlink(f.work.join("pipe.silico"), f.work.join("link.silico")).unwrap();
    for path in ["pipe.silico", "link.silico"] {
        for create in [false, true] {
            let answer = promptly(
                calling(&f.sessions, "open", &open_request(path, create)),
                path,
            );
            let (_, _, parameter) = error(&answer);
            let expected = if create { "create" } else { "path" };
            assert_eq!(parameter.as_deref(), Some(expected), "{path} {create}");
        }
    }
    // A render's `to` at a FIFO: refused, nothing written into it.
    f.file("part.silico", BOX);
    let open = f.open("part.silico", false);
    assert!(open.ok, "{}", open.text);
    let s = envelope(&open)["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    mkfifo(&f.work.join("view.png"));
    let render = promptly(
        calling(
            &f.sessions,
            "render",
            &format!(
                r#"{{"session": "{s}", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": "view.png", "replace": true}}"#
            ),
        ),
        "render",
    );
    assert_eq!(error(&render).2.as_deref(), Some("to"));
    // The session's file replaced by a FIFO after open: read again by the next command,
    // refused at once.
    std::fs::remove_file(f.work.join("part.silico")).unwrap();
    mkfifo(&f.work.join("part.silico"));
    for (name, arguments) in [
        (
            "text",
            format!(r#"{{"session": "{s}", "feature": null, "revision": null}}"#),
        ),
        (
            "check",
            format!(r#"{{"session": "{s}", "model": "current", "fresh": false}}"#),
        ),
        (
            "set_field",
            format!(
                r#"{{"session": "{s}", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}}"#
            ),
        ),
    ] {
        let _ = promptly(calling(&f.sessions, name, &arguments), name);
    }
}

#[test]
fn a_workspace_of_devices_opens_none_of_them() {
    // The core takes any directory as a root (only `silico serve` refuses `/`): a client
    // in `/dev` naming a device is refused at once, never read without end.
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions_in(dir.path(), Path::new("/dev"));
    for (path, create) in [
        ("zero", false),
        ("random", false),
        ("null", false),
        ("stdin", false),
        ("fd/0", false),
        ("tty", false),
        ("null", true),
    ] {
        let answer = promptly(
            calling(&sessions, "open", &open_request(path, create)),
            path,
        );
        let (code, _, _) = error(&answer);
        assert!(
            matches!(code.as_str(), "io_failed" | "invalid_input"),
            "{path}: {code}"
        );
    }
}

#[test]
fn directories_empty_paths_and_dots_are_refused_naming_the_path() {
    let f = fixture();
    std::fs::create_dir(f.work.join("sub")).unwrap();
    for path in [
        "",
        ".",
        "./",
        "sub",
        "sub/",
        "..",
        "../",
        "/",
        ".silico-cache",
    ] {
        let answer = f.open(path, false);
        let (code, message, parameter) = error(&answer);
        assert_eq!(parameter.as_deref(), Some("path"), "{path:?}: {message}");
        assert!(
            matches!(code.as_str(), "io_failed" | "invalid_input"),
            "{path:?}: {code}"
        );
    }
    for path in ["", ".", "./", "sub", "sub/", "..", "/"] {
        let answer = f.open(path, true);
        let (_, message, parameter) = error(&answer);
        assert!(
            matches!(parameter.as_deref(), Some("path" | "create")),
            "{path:?}: {message}"
        );
    }
}

#[test]
fn a_path_that_names_a_directory_makes_no_file() {
    // Broke (break-it): a path to make ending in `/.` or `/` names a directory, which
    // POSIX's `open(path, O_CREAT | O_EXCL)` refuses (EISDIR, or ENOENT when it's missing);
    // `made` took `Path::file_name` of it, which skips the `.` and the slash, so `brand/.`
    // made a file named `brand`, and `fresh.silico/` one named `fresh.silico`.
    let f = fixture();
    for (path, made) in [("brand/.", "brand"), ("fresh.silico/", "fresh.silico")] {
        let answer = f.open(path, true);
        assert!(
            !answer.ok,
            "{path:?} made {made}: {}",
            &answer.text[..answer.text.len().min(300)]
        );
        assert!(!f.work.join(made).exists(), "{path:?} made {made}");
        let e = &envelope(&answer)["errors"][0];
        assert_eq!(e["parameter"], "path", "{e}");
        assert_eq!(
            e["message"],
            format!("\"{path}\" names no file to make"),
            "{e}"
        );
    }
    // And a path whose last name is `.`-led, or a name in a directory, is still made.
    for (path, made) in [
        (".hidden.silico", ".hidden.silico"),
        ("sub/./x.silico", "sub/x.silico"),
    ] {
        std::fs::create_dir_all(f.work.join("sub")).unwrap();
        let answer = f.open(path, true);
        assert!(answer.ok, "{path}: {}", answer.text);
        assert!(f.work.join(made).is_file(), "{path}");
    }
}

#[test]
fn dot_dot_that_comes_back_inside_opens_and_makes_inside() {
    let f = fixture();
    f.file("part.silico", BOX);
    std::fs::create_dir(f.work.join("sub")).unwrap();
    let mut session = 0;
    for path in [
        "../work/part.silico",
        "sub/../part.silico",
        "./sub/./../part.silico",
        ".//part.silico",
        "sub/../../work/sub/../part.silico",
    ] {
        let open = f.open(path, false);
        assert!(open.ok, "{path}: {}", open.text);
        session += 1;
        assert_eq!(envelope(&open)["result"]["path"], path);
        let close = f.call("close", &format!(r#"{{"session": "s{session}"}}"#));
        assert!(close.ok, "{}", close.text);
    }
    for (path, at) in [
        ("../work/new.silico", "new.silico"),
        ("sub/../made.silico", "made.silico"),
        ("sub/./deep.silico", "sub/deep.silico"),
    ] {
        let made = f.open(path, true);
        assert!(made.ok, "{path}: {}", made.text);
        assert!(f.work.join(at).is_file(), "{path}");
    }
    // Through a directory that isn't there, as POSIX resolves it: can't be reached, not out
    // of the workspace.
    let missing = f.open("missing/../part.silico", false);
    let (code, message, _) = error(&missing);
    assert_eq!(code, "io_failed", "{message}");
    assert_eq!(f.theirs(), OUTSIDE);
}

#[test]
fn case_variants_of_a_file_inside_are_the_file_inside() {
    let f = fixture();
    f.file("part.silico", BOX);
    std::fs::write(f.work.join("Probe"), "").unwrap();
    if !f.work.join("probe").exists() {
        // A case-sensitive file system: the variants are other files.
        return;
    }
    let open = f.open("PART.SILICO", false);
    assert!(open.ok, "{}", open.text);
    // The same file by its own spelling, or the root's in another case: held by s1, never
    // outside.
    let root_variant = f.root.join("WORK/part.silico").to_str().unwrap().to_owned();
    for path in ["part.silico", "Part.Silico", root_variant.as_str()] {
        let again = f.open(path, false);
        let (_, message, parameter) = error(&again);
        assert!(
            !message.contains("outside the workspace"),
            "{path}: {message}"
        );
        assert_eq!(parameter.as_deref(), Some("path"), "{path}");
    }
    // A render's `to` naming the held document in another case: refused, the file kept.
    let render = f.render_to("s1", "Part.silico");
    assert_eq!(error(&render).2.as_deref(), Some("to"));
    assert_eq!(
        std::fs::read_to_string(f.work.join("part.silico")).unwrap(),
        BOX
    );
}

#[test]
fn unicode_spellings_of_the_root_are_inside_it() {
    // A root named in NFC, given back in NFD (macOS's file systems take either for one name).
    let dir = tempfile::tempdir().unwrap();
    let scratch = std::fs::canonicalize(dir.path()).unwrap();
    let nfc = scratch.join("caf\u{e9}");
    std::fs::create_dir(&nfc).unwrap();
    std::fs::write(nfc.join("part.silico"), BOX).unwrap();
    let nfd = scratch.join("cafe\u{301}");
    if !nfd.join("part.silico").exists() {
        // A file system that tells them apart: another directory.
        return;
    }
    let sessions = sessions_in(&scratch, &nfc);
    let open = call(
        &sessions,
        "open",
        &open_request(nfd.join("part.silico").to_str().unwrap(), false),
    );
    if !open.ok {
        let message = error(&open).1;
        assert!(!message.contains("outside the workspace"), "{message}");
    }
    // And a workspace given in NFD holds the NFC spelling.
    let sessions = sessions_in(&scratch, &nfd);
    let open = call(
        &sessions,
        "open",
        &open_request(nfc.join("part.silico").to_str().unwrap(), false),
    );
    if !open.ok {
        let message = error(&open).1;
        assert!(!message.contains("outside the workspace"), "{message}");
    }
}

#[test]
fn nul_and_very_long_paths_are_refused_as_data() {
    let f = fixture();
    f.file("part.silico", BOX);
    assert!(f.open("part.silico", false).ok);
    let long_name = format!("{}.silico", "n".repeat(300));
    let deep = format!("{}x.silico", "d/".repeat(3000));
    let long_out = format!("../{}x.silico", "d/".repeat(3000));
    for path in [
        "a\u{0}b.silico",
        "\u{0}",
        "part.silico\u{0}",
        long_name.as_str(),
        deep.as_str(),
        long_out.as_str(),
    ] {
        let shown = &path[..path.len().min(40)];
        for create in [false, true] {
            let answer = f.open(path, create);
            let (_, message, parameter) = error(&answer);
            assert!(
                matches!(parameter.as_deref(), Some("path" | "create")),
                "{shown:?} {create}: {message}"
            );
            assert!(answer.text.len() < REFUSAL_BOUND, "{shown:?} {create}");
        }
        let render = f.render_to("s1", path);
        let (_, message, parameter) = error(&render);
        assert_eq!(parameter.as_deref(), Some("to"), "{shown:?}: {message}");
    }
    // `part.silico\0` is never the held document, nor a way to write over it.
    assert_eq!(
        std::fs::read_to_string(f.work.join("part.silico")).unwrap(),
        BOX
    );
}

#[test]
fn an_outside_directory_is_never_told_apart_by_whether_it_exists_however_deep() {
    // Broke (break-it): `Workspace::leads_out` spent its budget of 40 links on each
    // directory it walked up as well as on each link it followed: so past about 40 missing
    // directories below an outside one, whether that one exists decided the answer.
    // `../outside/` (there) with 39 missing below it was "outside the workspace";
    // `../nowhere/` (not there) with 39 was "can't be reached": an MCP client learned which
    // directories exist outside the workspace, which the design says it never does. And
    // past 40 either way it wasn't called outside at all. Now only links spend the budget.
    let f = fixture();
    f.file("part.silico", BOX);
    assert!(f.open("part.silico", false).ok);
    for depth in [0, 1, 10, 38, 39, 40, 41, 60] {
        let missing = "m/".repeat(depth);
        for top in ["outside", "nowhere"] {
            let path = format!("../{top}/{missing}x.silico");
            let open = f.open(&path, false);
            let made = f.open(&path, true);
            let render = f.render_to("s1", &format!("../{top}/{missing}x.png"));
            for (what, answer) in [("open", &open), ("create", &made), ("render", &render)] {
                assert!(
                    is_outside(answer),
                    "{what} {top} at depth {depth}: {}",
                    error(answer).1
                );
            }
        }
    }
    assert!(!f.outside.join("m").exists());
}

#[test]
fn link_loops_and_long_chains_are_refused_promptly_and_write_nothing_outside() {
    let f = fixture();
    f.file("part.silico", BOX);
    assert!(f.open("part.silico", false).ok);
    // a → b → a.
    std::os::unix::fs::symlink("b.silico", f.work.join("a.silico")).unwrap();
    std::os::unix::fs::symlink("a.silico", f.work.join("b.silico")).unwrap();
    // l0.png → l1.png → ... → l59.png → outside/made.png, which isn't there.
    let target = f.outside.join("made.png");
    for i in 0..60 {
        let to = if i == 59 {
            target.clone()
        } else {
            PathBuf::from(format!("l{}.png", i + 1))
        };
        std::os::unix::fs::symlink(to, f.work.join(format!("l{i}.png"))).unwrap();
    }
    // A link straight out, to a file not there.
    std::os::unix::fs::symlink(&target, f.work.join("out.png")).unwrap();
    for path in [
        "a.silico",
        "a.silico/x.silico",
        "l0.png",
        "l58.png",
        "out.png",
    ] {
        let open = promptly(
            calling(&f.sessions, "open", &open_request(path, false)),
            path,
        );
        assert_eq!(error(&open).2.as_deref(), Some("path"), "{path}");
        let made = promptly(
            calling(&f.sessions, "open", &open_request(path, true)),
            path,
        );
        assert!(!made.ok, "{path}: {}", made.text);
        let render = f.render_to("s1", path);
        assert_eq!(error(&render).2.as_deref(), Some("to"), "{path}");
    }
    // The links straight out are refused as out of the workspace, which they are.
    assert!(is_outside(&f.render_to("s1", "out.png")));
    assert!(is_outside(&f.render_to("s1", "l58.png")));
    assert!(!target.exists());
    assert_eq!(std::fs::read_dir(&f.outside).unwrap().count(), 1);
}

#[test]
fn the_root_swapped_for_a_link_out_mid_session_is_refused_as_the_file_is_read_again() {
    let f = fixture();
    f.file("part.silico", BOX);
    assert!(f.open("part.silico", false).ok);
    assert!(
        f.call(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#
        )
        .ok
    );
    // The workspace moved aside, and a link out put where it was.
    std::fs::rename(&f.work, f.root.join("work.real")).unwrap();
    std::os::unix::fs::symlink(&f.outside, &f.work).unwrap();
    for (name, arguments) in [
        (
            "text",
            r#"{"session": "s1", "feature": null, "revision": null}"#,
        ),
        (
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
        ),
        (
            "format",
            r#"{"session": "s1", "features": null, "write": true}"#,
        ),
    ] {
        let answer = f.call(name, arguments);
        assert!(is_outside(&answer), "{name}: {}", answer.text);
        assert_eq!(error(&answer).2.as_deref(), Some("path"), "{name}");
    }
    // And a new open through the root's path: out of the workspace, as it now leads.
    assert!(is_outside(&f.open("part.silico", false)));
    assert!(is_outside(&f.open("new.silico", true)));
    assert!(is_outside(&f.render_to("s1", "view.png")));
    assert_eq!(f.theirs(), OUTSIDE);
    assert_eq!(std::fs::read_dir(&f.outside).unwrap().count(), 1);
}

#[test]
fn a_cache_directory_swapped_for_a_link_out_mid_session_keeps_nothing_through_it() {
    let f = fixture();
    f.file("part.silico", BOX);
    assert!(f.open("part.silico", false).ok);
    let check = r#"{"session": "s1", "model": "current", "fresh": false}"#;
    assert!(f.call("check", check).ok);
    let cache = f.work.join(".silico-cache");
    assert!(cache.is_dir(), "the cache was never made");
    // Moved aside, and a link out put in its place.
    std::fs::rename(&cache, f.work.join("old-cache")).unwrap();
    let theirs = f.outside.join("cache");
    std::fs::create_dir(&theirs).unwrap();
    std::os::unix::fs::symlink(&theirs, &cache).unwrap();
    for (revision, length) in [(1, 41), (2, 42), (3, 43)] {
        let edit = f.call(
            "set_field",
            &format!(
                r#"{{"session": "s1", "revision": {revision}, "feature": "k7f2qx3m", "field": "length", "value": {length}}}"#
            ),
        );
        assert!(edit.ok, "{}", edit.text);
        assert!(f.call("check", check).ok);
    }
    // A second session in the same directory, opened since the swap.
    f.file("second.silico", BOX);
    assert!(f.open("second.silico", false).ok);
    assert!(
        f.call(
            "check",
            r#"{"session": "s2", "model": "current", "fresh": false}"#
        )
        .ok
    );
    assert_eq!(
        std::fs::read_dir(&theirs).unwrap().count(),
        0,
        "the cache was written through the link out"
    );
}

#[test]
fn a_renders_to_inside_by_any_spelling_writes_there_and_nowhere_else() {
    let f = fixture();
    f.file("part.silico", BOX);
    std::fs::create_dir(f.work.join("sub")).unwrap();
    assert!(f.open("part.silico", false).ok);
    for (to, at) in [
        ("sub/../a.png", "a.png"),
        ("../work/b.png", "b.png"),
        ("./sub/./c.png", "sub/c.png"),
    ] {
        let render = f.render_to("s1", to);
        assert!(render.ok, "{to}: {}", render.text);
        assert!(f.work.join(at).is_file(), "{to}");
    }
    for to in [
        "",
        ".",
        "sub",
        "sub/",
        ".silico-cache/x.png",
        "part.silico",
        "./part.silico",
    ] {
        let render = f.render_to("s1", to);
        assert_eq!(error(&render).2.as_deref(), Some("to"), "{to:?}");
    }
    assert_eq!(
        std::fs::read_to_string(f.work.join("part.silico")).unwrap(),
        BOX
    );
}

/// The refusal of `given`, in the request's `field`, as out of the workspace: its code,
/// message, and parameter (M4.1 section 7's `open` example).
fn outside(given: &str, field: &str) -> (String, String, Option<String>) {
    (
        "invalid_input".to_owned(),
        format!("\"{given}\" is outside the workspace this server was started in"),
        Some(field.to_owned()),
    )
}

/// A render of s1's current model, written to `to`, replacing a file there if `replace`.
fn render(
    sessions: &silico_core::command::Sessions<std::sync::Arc<silico_core::fake::FakeKernel>>,
    to: &str,
    replace: bool,
) -> Answered {
    call(
        sessions,
        "render",
        &format!(
            r#"{{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 40, "height": 30, "highlight": [], "to": {}, "replace": {replace}}}"#,
            serde_json::to_string(to).unwrap()
        ),
    )
}

#[test]
fn a_path_through_an_outside_directory_and_back_is_never_told_apart_by_its_existence() {
    // Found by the review: a path was resolved by the system before it was checked, so
    // `../secrets/../work/part.silico` opened when `secrets` was there (the system went into
    // it and back) and was refused as outside when it wasn't: whether anything outside is
    // there told by the answer. Now the walk never looks a name up in a directory outside
    // the workspace, and both are outside, as is every way of spelling them.
    let f = fixture();
    f.opened();
    std::fs::create_dir(f.root.join("secrets")).unwrap();
    let root = f.root.to_str().unwrap();
    for top in ["secrets", "nothere"] {
        for (path, ending) in [
            (format!("../{top}/../work/part.silico"), "part.silico"),
            (format!("{root}/{top}/../work/part.silico"), "part.silico"),
            (
                format!("../{top}/../work/sub/../part.silico"),
                "part.silico",
            ),
            (
                format!("../work/../{top}/../work/part.silico"),
                "part.silico",
            ),
        ] {
            assert_eq!(error(&f.open(&path, false)), outside(&path, "path"));
            let made = path.replace(ending, "made.silico");
            assert_eq!(error(&f.open(&made, true)), outside(&made, "path"));
            let to = path.replace(ending, "view.png");
            assert_eq!(error(&f.render_to("s1", &to)), outside(&to, "to"));
        }
    }
    // Nothing made, nothing written, in the workspace or out of it.
    assert!(!f.work.join("made.silico").exists());
    assert!(!f.work.join("view.png").exists());
    assert_eq!(
        std::fs::read_dir(f.root.join("secrets")).unwrap().count(),
        0
    );
    assert!(!f.root.join("nothere").exists());
    // Coming back down through the root's own ancestors is no way out: it opens (as s2,
    // the file held by s1 refused as held, not as outside).
    let back = f.open(&format!("{root}/work/../work/part.silico"), false);
    assert!(!error(&back).1.contains("outside"), "{}", back.text);
}

#[test]
fn a_path_through_a_file_and_back_up_is_never_told_apart_by_whats_outside() {
    // Found by the review (round 2): the walk stopped at a file in the workspace and left the
    // rest to the system, and macOS's `realpath` takes `part.silico/..` by its text and goes
    // on, so `part.silico/../../secrets/x` was looked up outside: refused as outside when
    // there, as not found when not. Now a file with more of the path to go stops the walk,
    // as glibc stops it: not a directory, whatever is outside.
    let f = fixture();
    f.opened();
    std::fs::create_dir(f.root.join("secrets")).unwrap();
    std::fs::write(f.root.join("secrets/x.silico"), BOX).unwrap();
    let answers: Vec<Vec<(String, String, Option<String>)>> = ["secrets", "nothere"]
        .iter()
        .map(|top| {
            let open = format!("part.silico/../../{top}/x.silico");
            let made = format!("part.silico/../../{top}/made.silico");
            let to = format!("part.silico/../../{top}/view.png");
            [
                error(&f.open(&open, false)),
                error(&f.open(&made, true)),
                error(&f.render_to("s1", &to)),
            ]
            .into_iter()
            .map(|(code, message, parameter)| (code, message.replace(top, "TOP"), parameter))
            .collect()
        })
        .collect();
    assert_eq!(answers[0], answers[1]);
    // Each as a path the system can't follow (open's io_failed, render's to refused as
    // unreachable), never as outside: nothing outside was looked at to answer it.
    for (code, message, _) in &answers[0] {
        assert!(message.contains("Not a directory"), "{code}: {message}");
        assert!(!message.contains("outside"), "{code}: {message}");
    }
    assert!(!f.root.join("secrets/made.silico").exists());
    assert!(!f.root.join("secrets/view.png").exists());
}

#[test]
fn a_path_through_a_directory_this_user_cant_search_is_outside_never_denied() {
    // Nothing outside the workspace is looked in, so a directory there that would refuse
    // the lookup (or block on it: a mount made on demand, a stale network share) is never
    // asked: the path is outside, as it is wherever it goes, not "permission denied".
    use std::os::unix::fs::PermissionsExt as _;
    let f = fixture();
    f.opened();
    let locked = f.root.join("locked");
    std::fs::create_dir_all(locked.join("in")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let answers: Vec<_> = [
        "../locked/../work/part.silico",
        "../locked/in/part.silico",
        "../locked/part.silico",
    ]
    .iter()
    .map(|path| {
        (
            path.to_string(),
            error(&f.open(path, false)),
            error(&f.open(&path.replace("part.silico", "made.silico"), true)),
            error(&f.render_to("s1", &path.replace("part.silico", "view.png"))),
        )
    })
    .collect();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    for (path, open, made, render) in answers {
        assert_eq!(open, outside(&path, "path"));
        assert_eq!(
            made,
            outside(&path.replace("part.silico", "made.silico"), "path")
        );
        assert_eq!(
            render,
            outside(&path.replace("part.silico", "view.png"), "to")
        );
    }
}

#[test]
fn a_renders_to_that_is_a_link_is_resolved_as_every_path_is() {
    // mutants (cargo-mutants, the link at `to` out of the workspace): each link at a
    // render's `to` refused, or written through, as the rules say.
    let f = fixture();
    f.opened();
    std::fs::write(f.outside.join("theirs.png"), b"theirs").unwrap();
    std::fs::write(f.work.join("real.png"), b"ours").unwrap();
    let link = |name: &str, to: &Path| std::os::unix::fs::symlink(to, f.work.join(name)).unwrap();
    link("out.png", &f.outside.join("theirs.png"));
    link("dangling-out.png", &f.outside.join("none.png"));
    link("in.png", &f.work.join("real.png"));
    link("dangling-in.png", &f.work.join("none.png"));
    // Out, to a file there or to nothing: outside, replacing or not.
    for to in ["out.png", "dangling-out.png"] {
        for replace in [false, true] {
            assert_eq!(
                error(&render(&f.sessions, to, replace)),
                outside(to, "to"),
                "{to} {replace}"
            );
        }
    }
    assert_eq!(
        std::fs::read(f.outside.join("theirs.png")).unwrap(),
        b"theirs"
    );
    assert!(!f.outside.join("none.png").exists());
    // In, to a file there: there already, unless replaced, then written through the link.
    assert_eq!(
        error(&render(&f.sessions, "in.png", false)),
        (
            "invalid_input".to_owned(),
            "\"in.png\" exists".to_owned(),
            Some("to".to_owned())
        )
    );
    assert!(render(&f.sessions, "in.png", true).ok);
    assert_eq!(
        std::fs::read(f.work.join("real.png")).unwrap(),
        b"PNG 40x30"
    );
    assert!(f.work.join("in.png").is_symlink());
    // In, to nothing: a link to nothing, not outside.
    let (code, message, parameter) = error(&render(&f.sessions, "dangling-in.png", true));
    assert_eq!(
        (code.as_str(), parameter.as_deref()),
        ("invalid_input", Some("to"))
    );
    assert!(
        message.starts_with("to \"dangling-in.png\": is a link to nothing that can be reached: "),
        "{message}"
    );
    assert!(f.work.join("dangling-in.png").is_symlink());
    assert!(!f.work.join("none.png").exists());
}

#[test]
fn without_a_workspace_a_renders_to_that_is_a_link_is_written_through_it_anywhere() {
    // The CLI's rules: no workspace, so a link anywhere is written through, and a link to
    // nothing is refused as one.
    let f = fixture();
    let (_, sessions, _) = crate::calls::sessions_on(&f.root, |_| {});
    let part = f.file("part.silico", BOX);
    let open = call(
        &sessions,
        "open",
        &open_request(part.to_str().unwrap(), false),
    );
    assert!(open.ok, "{}", open.text);
    std::fs::write(f.outside.join("theirs.png"), b"theirs").unwrap();
    let out = f.work.join("out.png");
    std::os::unix::fs::symlink(f.outside.join("theirs.png"), &out).unwrap();
    let dangling = f.work.join("dangling.png");
    std::os::unix::fs::symlink(f.outside.join("none.png"), &dangling).unwrap();
    let out = out.to_str().unwrap();
    assert_eq!(
        error(&render(&sessions, out, false)),
        (
            "invalid_input".to_owned(),
            format!("\"{out}\" exists"),
            Some("to".to_owned())
        )
    );
    let written = render(&sessions, out, true);
    assert!(written.ok, "{}", written.text);
    assert_eq!(
        std::fs::read(f.outside.join("theirs.png")).unwrap(),
        b"PNG 40x30"
    );
    let dangling = dangling.to_str().unwrap();
    let (_, message, parameter) = error(&render(&sessions, dangling, true));
    assert_eq!(parameter.as_deref(), Some("to"));
    assert!(
        message.starts_with(&format!(
            "to \"{dangling}\": is a link to nothing that can be reached: "
        )),
        "{message}"
    );
    assert!(!f.outside.join("none.png").exists());
}
