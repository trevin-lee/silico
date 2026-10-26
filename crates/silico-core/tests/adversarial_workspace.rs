//! Hostile paths against a workspace (docs/design/m4.1-command-api.md, decision 26;
//! docs/design/m4.9-mcp.md, *What's added*): a root that's a link, removed after start, or
//! swapped for a link mid-session; FIFOs and devices where a document or a render's `to` is
//! looked for; directories, empty paths, `.`, `..` that leads back inside, NUL, very long
//! paths, case variants, link loops and chains, outside directories deeper than the links
//! the check follows; and a cache directory swapped for a link out mid-session. Expected
//! values: POSIX (what `open(2)` of the same path does), the design's rule ("a path out of
//! the workspace is refused as one, whether or not anything is there"), and the files'
//! bytes as the tests wrote them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Calling, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer};
use tempfile::TempDir;

use crate::sessions::HANG;

/// A 40 × 20 × 10 box.
const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// Another box, the file outside the workspace.
const OUTSIDE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Theirs", "position": [0, 0, 0], "length": 30, "width": 20, "height": 10}]}"#;

/// What a refusal's envelope may reasonably be.
const REFUSAL_BOUND: usize = 16 << 10;

/// The longest a call that refuses a FIFO or a device may take: it never waits on one.
const PROMPT: Duration = Duration::from_secs(10);

/// A scratch directory holding the workspace, `work`, and `outside` beside it (with
/// `part.silico`, [`OUTSIDE`]); sessions on a fake kernel and a fake renderer in `work`.
struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    work: PathBuf,
    outside: PathBuf,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn sessions_in(scratch: &Path, workspace: &Path) -> Sessions<Arc<FakeKernel>> {
    let secret = scratch.join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    Sessions::with_cache_opener(kernel, move |cache| Cache::with_secret(cache, &secret))
        .rendering(FakeRenderer::new())
        .in_workspace(workspace)
        .unwrap()
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let work = root.join("work");
    let outside = root.join("outside");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("part.silico"), OUTSIDE).unwrap();
    let sessions = sessions_in(&root, &work);
    Fixture {
        _dir: dir,
        root,
        work,
        outside,
        sessions,
    }
}

impl Fixture {
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.work.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn call(&self, name: &str, arguments: &str) -> Answered {
        call(&self.sessions, name, arguments)
    }

    fn open(&self, path: &str, create: bool) -> Answered {
        self.call("open", &open_request(path, create))
    }

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

    /// The outside file's bytes, which no test may change.
    fn theirs(&self) -> String {
        std::fs::read_to_string(self.outside.join("part.silico")).unwrap()
    }
}

fn open_request(path: &str, create: bool) -> String {
    format!(
        r#"{{"path": {}, "create": {create}}}"#,
        serde_json::to_string(path).unwrap()
    )
}

fn calling(sessions: &Sessions<Arc<FakeKernel>>, name: &str, arguments: &str) -> Calling {
    match sessions.call(name, arguments) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("{name} is no command"),
    }
}

fn call(sessions: &Sessions<Arc<FakeKernel>>, name: &str, arguments: &str) -> Answered {
    match calling(sessions, name, arguments).wait_timeout(HANG) {
        Ok(answered) => answered,
        Err(_) => panic!("{name} didn't answer in {HANG:?}"),
    }
}

/// The call's answer, failing if it takes longer than [`PROMPT`].
fn promptly(calling: Calling, what: &str) -> Answered {
    match calling.wait_timeout(PROMPT) {
        Ok(answered) => answered,
        Err(_) => panic!("{what} didn't answer in {PROMPT:?}: it waits on a FIFO or a device"),
    }
}

fn envelope(answered: &Answered) -> serde_json::Value {
    serde_json::from_str(&answered.text).unwrap()
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
        ("text", format!(r#"{{"session": "{s}", "feature": null}}"#)),
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
    // BREAKS: `Workspace::leads_out` spends its budget of 40 links on each directory it
    // walks up as well as on each link it follows: so past about 40 missing directories
    // below an outside one, whether that one exists decides the answer. `../outside/` (there)
    // with 39 missing below it is "outside the workspace"; `../nowhere/` (not there) with 39
    // is "can't be reached": an MCP client learns which directories exist outside the
    // workspace, which the design says it never does. And past 40 either way it isn't
    // called outside at all.
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
        ("text", r#"{"session": "s1", "feature": null}"#),
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
