//! break-it for `silico fmt --merge-touching` (docs/design/m5.0-app.md, `sketch_merge`,
//! decision 17): a read-only file it has merges for, three ends that touch only through each
//! other, more clusters than `check` lists (a hundred) in one sketch, one cluster of more
//! points than a warning lists, and a file with nothing to merge under `--check`. Expected
//! values: the CLI's contract (exit 0 clean, 1 the document has problems, 2 the command
//! couldn't run; nothing written that isn't said), and by hand from the warning's rule
//! (ends strictly closer than 1e-7 mm, grouped through each other) and the merge's (the
//! first named kept at its own coordinates, the others' curves re-pointed to it).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/run.rs"]
mod run;

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("adv-merge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let file = self.0.join(name);
        std::fs::write(&file, text).unwrap();
        file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn silico(scratch: &Scratch, args: &[&str], file: &Path) -> Output {
    let exe = Path::new(env!("CARGO_BIN_EXE_silico"));
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .arg(file)
        .env(
            "SILICO_KERNEL_WORKER",
            exe.with_file_name("silico-kernel-worker"),
        )
        .env(
            "SILICO_SKETCH_WORKER",
            exe.with_file_name("silico-sketch-worker"),
        )
        .env("XDG_CACHE_HOME", scratch.0.join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::null());
    run::output(cmd)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// The 8-character ID `prefix` then `n` in base 32, as members' IDs are written.
fn id(prefix: char, n: usize) -> String {
    let mut digits = Vec::new();
    let mut n = n;
    for _ in 0..7 {
        digits.push(ALPHABET[n % 32] as char);
        n /= 32;
    }
    digits.reverse();
    format!("{prefix}{}", digits.into_iter().collect::<String>())
}

/// A document of one sketch on xy of `points` (ID, x, y, as written) and `lines`, no
/// constraints.
fn sketch(points: &[(String, String, String)], lines: &[(String, String, String)]) -> String {
    let mut p = String::new();
    for (i, (id, x, y)) in points.iter().enumerate() {
        let sep = if i == 0 { "" } else { ", " };
        write!(
            p,
            r#"{sep}{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#
        )
        .unwrap();
    }
    let mut l = String::new();
    for (i, (id, from, to)) in lines.iter().enumerate() {
        let sep = if i == 0 { "" } else { ", " };
        write!(
            l,
            r#"{sep}{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#
        )
        .unwrap();
    }
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [
{{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{p}], "lines": [{l}], "circles": [], "arcs": [], "constraints": []}}
]}}
"#
    )
}

fn s(text: &str) -> String {
    text.to_owned()
}

/// Two lines whose ends at (10, 0) and (10.00000004, 0) touch: one cluster.
fn one_cluster() -> String {
    sketch(
        &[
            (s("pa000001"), s("0"), s("0")),
            (s("pa000002"), s("10"), s("0")),
            (s("pa000003"), s("10.00000004"), s("0")),
            (s("pa000004"), s("10.00000004"), s("10")),
        ],
        &[
            (s("na000001"), s("pa000001"), s("pa000002")),
            (s("na000002"), s("pa000003"), s("pa000004")),
        ],
    )
}

/// The `ends_not_joined` warnings `silico check` says of `file`.
fn warned(scratch: &Scratch, file: &Path) -> usize {
    let out = silico(scratch, &["check"], file);
    stderr(&out)
        .lines()
        .filter(|line| line.contains("warning: ends_not_joined"))
        .count()
}

/// The lines (from, to) of the sketch in `text`, as it's written.
fn lines_of(text: &str) -> Vec<(String, String)> {
    let document: serde_json::Value = serde_json::from_str(text).unwrap();
    document["features"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["from"].as_str().unwrap().to_owned(),
                l["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_read_only_file_with_a_merge_to_make_is_left_and_said_as_unwritable_only() {
    // The merge is made in the session's transaction, and the commit can't write: exit 2,
    // the file as it was, and the reason said. The transaction is the CLI's own, so its
    // user has nothing to commit or roll back.
    // BREAKS: after the commit's io_failed, the CLI closes the session with the transaction
    // still open, and says "invalid_input: a transaction is open in session s1 (parameter
    // session). Suggestion: commit or roll back the transaction, then close the session.":
    // a session and a transaction the user never made, with advice no CLI user can take.
    let scratch = Scratch::new("read-only");
    let text = one_cluster();
    let file = scratch.file("part.silico", &text);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let out = silico(&scratch, &["fmt", "--merge-touching"], &file);
    let said = stderr(&out);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(out.status.code(), Some(2), "{said}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    assert!(said.contains("io_failed"), "{said}");
    assert!(!said.contains("transaction"), "{said}");
    assert!(!said.contains("session"), "{said}");
}

#[test]
fn three_ends_touching_only_through_each_other_merge_into_the_least_in_one_run() {
    // Ends at x = 10, 10 + 6e-8, and 10 + 1.2e-7: the first and last are 1.2e-7 apart, past
    // the joining distance, but each is within it of the middle one, so the warning's one
    // cluster holds all three, and one merge makes them pa000002, at (10, 0).
    let scratch = Scratch::new("chain");
    let text = sketch(
        &[
            (s("pa000001"), s("0"), s("0")),
            (s("pa000002"), s("10"), s("0")),
            (s("pa000003"), s("10.00000006"), s("0")),
            (s("pa000004"), s("10.00000006"), s("10")),
            (s("pa000005"), s("10.00000012"), s("0")),
            (s("pa000006"), s("20"), s("0")),
        ],
        &[
            (s("na000001"), s("pa000001"), s("pa000002")),
            (s("na000002"), s("pa000003"), s("pa000004")),
            (s("na000003"), s("pa000005"), s("pa000006")),
        ],
    );
    let file = scratch.file("part.silico", &text);
    assert_eq!(warned(&scratch, &file), 1);
    let out = silico(&scratch, &["fmt", "--merge-touching"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            ": merged points pa000002, pa000003, pa000005 of sketch s0000001, keeping pa000002"
        ),
        "{}",
        stderr(&out)
    );
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        lines_of(&written),
        [
            (s("pa000001"), s("pa000002")),
            (s("pa000002"), s("pa000004")),
            (s("pa000002"), s("pa000006")),
        ]
    );
    assert!(written.contains(r#"{"id": "pa000002", "x": 10, "y": 0, "construction": false}"#));
    assert_eq!(warned(&scratch, &file), 0);
    // A second run changes nothing.
    let out = silico(&scratch, &["fmt", "--merge-touching"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), written);
}

#[test]
fn more_clusters_than_check_lists_are_all_merged_and_two_copies_write_the_same_bytes() {
    // 120 corners, each two lines whose ends meet at one place: 120 clusters, of which
    // check lists the first 100, so a second round merges the rest. 960 unknowns, under
    // the sketch solver's 1024.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for k in 0..120 {
        let x = 30 * k;
        let [a, b, c, d] = [0, 1, 2, 3].map(|i| id('p', 4 * k + i));
        points.extend([
            (a.clone(), format!("{x}"), s("0")),
            (b.clone(), format!("{}", x + 10), s("0")),
            (c.clone(), format!("{}", x + 10), s("0")),
            (d.clone(), format!("{}", x + 10), s("10")),
        ]);
        lines.push((id('n', 2 * k), a, b));
        lines.push((id('n', 2 * k + 1), c, d));
    }
    let text = sketch(&points, &lines);
    let scratch = Scratch::new("many");
    let first = scratch.file("a.silico", &text);
    let second = scratch.file("b.silico", &text);
    for file in [&first, &second] {
        let out = silico(&scratch, &["fmt", "--merge-touching"], file);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let merged = stderr(&out)
            .lines()
            .filter(|l| l.contains(": merged points"))
            .count();
        assert_eq!(merged, 120, "{}", stderr(&out));
    }
    let written = std::fs::read_to_string(&first).unwrap();
    assert_eq!(written, std::fs::read_to_string(&second).unwrap());
    assert_eq!(warned(&scratch, &first), 0);
    // Each corner's second line starts at the first's end, the least ID of the two.
    let joined = lines_of(&written);
    for k in 0..120 {
        assert_eq!(joined[2 * k + 1].0, id('p', 4 * k + 1), "corner {k}");
    }
}

#[test]
fn a_cluster_of_more_points_than_a_warning_lists_is_merged_whole_into_its_least() {
    // 130 lines from the origin outward: 130 ends at one place, one cluster, of which the
    // warning names the first 100. Merged in two rounds, every line starts at p0000000.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for k in 0..130 {
        let angle = std::f64::consts::TAU * k as f64 / 130.0;
        let (a, b) = (id('p', 2 * k), id('p', 2 * k + 1));
        points.push((a.clone(), s("0"), s("0")));
        points.push((
            b.clone(),
            format!("{:?}", (10.0 * libm::cos(angle) * 1e6).round() / 1e6),
            format!("{:?}", (10.0 * libm::sin(angle) * 1e6).round() / 1e6),
        ));
        lines.push((id('n', k), a, b));
    }
    let scratch = Scratch::new("star");
    let file = scratch.file("part.silico", &sketch(&points, &lines));
    let out = silico(&scratch, &["fmt", "--merge-touching"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let written = std::fs::read_to_string(&file).unwrap();
    let starts: Vec<String> = lines_of(&written)
        .into_iter()
        .map(|(from, _)| from)
        .collect();
    assert!(starts.iter().all(|from| from == "p0000000"), "{starts:?}");
    assert_eq!(warned(&scratch, &file), 0);
}

#[test]
fn check_merge_touching_on_a_file_with_nothing_to_merge_says_nothing_and_writes_nothing() {
    let scratch = Scratch::new("clean");
    let file = scratch.file("part.silico", &one_cluster());
    let out = silico(&scratch, &["fmt", "--merge-touching"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = std::fs::read_to_string(&file).unwrap();
    let out = silico(&scratch, &["fmt", "--check", "--merge-touching"], &file);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}
