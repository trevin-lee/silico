//! M6.5 on the real kernel: the `pattern` read (docs/design/m6.0-patterns.md, section 5).
//! The design's plate (section 2.2) rebuilt on kernel workers, its three patterns read and
//! written byte for byte as section 5's examples, each committed in silico-core's
//! `tests/contract/commands/examples/pattern/` (the core's tests check each is the design's
//! own text and holds to its schema); the places by hand from the document's numbers
//! (section 9's worked examples); references by name, read from the base's kept
//! description, giving what the origin's words give; and each state a step's verdict gives,
//! from section 4.2's and 4.3's cases. Sketches are solved as stored, by the fake sketch
//! solver.

use super::pattern_docs::{self, THROUGH, direction, down, linear_as, of, up};
use super::sweeps::{circles, id, pad, pocket, rectangle};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use silico_core::command::pattern::{InstanceState, PatternRead, read};
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::document::{Checked, Length};
use silico_core::fake::FakeSketches;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// Long enough for any rebuild here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(300);

fn engine() -> Recompute<KernelPool> {
    let pool = KernelPool::start_workers(Path::new(WORKER), 2).unwrap();
    Recompute::new(pool).with_sketches(FakeSketches::new())
}

/// The design's text between the headings `from` and `to`.
fn section(from: &str, to: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m6.0-patterns.md");
    let doc = std::fs::read_to_string(path).unwrap();
    let start = doc.find(from).unwrap();
    let end = start + doc[start..].find(to).unwrap();
    doc[start..end].to_owned()
}

/// The plate of section 2.2, as the design writes it.
fn plate_text() -> String {
    let section = section("### 2.2 The plate", "### 2.3 Other forms");
    let block = section.split("```json\n").nth(1).unwrap();
    format!("{}\n", block.split("\n```").next().unwrap())
}

/// `text`, a document, rebuilt and published on the real kernel.
fn rebuilt(text: &str) -> Arc<Model> {
    let checked = Checked::new(silico_core::document::read(text.as_bytes()).unwrap()).unwrap();
    match engine().start(checked).wait_timeout(HANG) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish"),
    }
}

/// `text` with `from` replaced by `to`, where `from` is written exactly once.
fn edited(text: &str, from: &str, to: &str) -> String {
    assert_eq!(text.matches(from).count(), 1, "{from}");
    text.replacen(from, to, 1)
}

/// The example `pattern/name`, as committed.
fn example(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/commands/examples/pattern")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

fn read_of(model: &Model, pattern: &str) -> PatternRead {
    read(model, &id(pattern)).unwrap_or_else(|p| panic!("{pattern}: {:?}", p.errors()))
}

fn point(p: [f64; 3]) -> [Length; 3] {
    p.map(|c| Length::rounded(c).unwrap())
}

/// Each instance's index, as its names write it, and its state.
fn states(read: &PatternRead) -> Vec<(String, Option<InstanceState>)> {
    read.instances
        .iter()
        .map(|i| (i.index.to_string(), i.state))
        .collect()
}

#[test]
fn the_plates_patterns_read_as_the_designs_examples_byte_for_byte() {
    let model = rebuilt(&plate_text());
    // Every feature built, the chamfer on copy (2, 1)'s rim among them.
    for (feature, outcome) in model.features() {
        assert!(
            matches!(
                outcome,
                FeatureOutcome::Built { .. } | FeatureOutcome::Sketch(_)
            ),
            "{feature}: {outcome:?}"
        );
    }
    for (pattern, name) in [
        ("t0000001", "result"),
        ("t0000002", "result-polar"),
        ("m0000001", "result-mirror"),
    ] {
        assert_eq!(
            written(&read_of(&model, pattern)),
            example(name),
            "{pattern}"
        );
    }
    // By hand (section 9): the corner hole r 2.75 at (−40, −20), stepped 80 / 2 = 40 along
    // +X and 40 / 1 = 40 along +Y, so instance (2, 1)'s centre is at (40, 20, 0); the boss
    // hole at (5, 0) turned 90°, 180°, and 270° about +Z, at (0, 5, 0), (−5, 0, 0), and
    // (0, −5, 0); the notch x 44 to 50, y −5 to 5, centred at (47, 0, 0), mirrored in x = 0
    // to (−47, 0, 0).
    let corner = read_of(&model, "t0000001");
    assert_eq!(corner.anchor, Some(point([-40.0, -20.0, 0.0])));
    assert_eq!(corner.instances[4].index.offsets(), [2, 1]);
    assert_eq!(corner.instances[4].anchor, Some(point([40.0, 20.0, 0.0])));
    let polar = read_of(&model, "t0000002");
    let turned: Vec<_> = polar.instances.iter().map(|i| i.anchor.unwrap()).collect();
    assert_eq!(
        turned,
        [
            point([0.0, 5.0, 0.0]),
            point([-5.0, 0.0, 0.0]),
            point([0.0, -5.0, 0.0])
        ]
    );
    let mirror = read_of(&model, "m0000001");
    assert_eq!(mirror.anchor, Some(point([47.0, 0.0, 0.0])));
    assert_eq!(mirror.instances[0].anchor, Some(point([-47.0, 0.0, 0.0])));
}

/// The plate to its corner holes' pattern, its direction 2 `along` and `reversed` as given.
fn grid(along: &str, reversed: bool) -> String {
    let plate = plate_text();
    let cut = plate.find("    {\n      \"id\": \"s0000004\"").unwrap();
    let text = format!(
        "{}\n  ]\n}}\n",
        plate[..cut].trim_end().trim_end_matches(',')
    );
    edited(
        &text,
        "\"along\": \"y\",\n          \"reversed\": false,",
        &format!("\"along\": {along},\n          \"reversed\": {reversed},"),
    )
}

#[test]
fn a_direction_along_a_faces_name_reads_as_its_outward_normal_its_sense_reversed() {
    // The plate's front face, swept from na000001 along y = −30, faces −Y outward; turned
    // round (`reversed`), +Y: the read is the design's example of `y` byte for byte, the
    // face read from the base's kept description (decision 3).
    let front = "\"p0000001.pad.Face(G s0000001.sketch.Edge.na000001)\"";
    let model = rebuilt(&grid(front, true));
    assert_eq!(written(&read_of(&model, "t0000001")), example("result"));
    // Not turned round: −Y, each instance of the second row moved 40 down to y = −60, off
    // the plate (y −30 to 30, the hole r 2.75): a pocket's copy that misses cuts nothing,
    // so those three changed nothing, by the step's history; the first row's are built.
    let model = rebuilt(&grid(front, false));
    let read = read_of(&model, "t0000001");
    let directions = read.directions.as_ref().unwrap();
    assert_eq!(directions[1].direction, [0.0, -1.0, 0.0]);
    assert_eq!(read.instances[4].offset, Some(point([80.0, -40.0, 0.0])));
    assert_eq!(read.instances[4].anchor, Some(point([40.0, -60.0, 0.0])));
    let changed = Some(InstanceState::ChangedNothing);
    let built = Some(InstanceState::Built);
    assert_eq!(
        states(&read),
        [
            ("0,1".to_owned(), changed),
            ("1".to_owned(), built),
            ("1,1".to_owned(), changed),
            ("2".to_owned(), built),
            ("2,1".to_owned(), changed),
        ]
    );
    // Built, with the warning, which isn't a problem.
    assert_eq!((read.problems.len(), read.problems_total), (0, 0));
}

#[test]
fn an_axis_on_a_circles_name_turns_about_its_centre_as_elements_describes_it() {
    // The boss's top rim, a circle r 8 about (0, 0, 15) (the boss padded 10 on the plate's
    // top at z = 5): the boss hole (5, 0) turned 45° about it, by spacing, lands at
    // 5 · (cos 45°, ±sin 45°, 0) as the rim's axis points ±Z.
    let rim = "p0000002.pad.Edge(G s0000004.sketch.Edge.cd000001)";
    let text = edited(
        &plate_text(),
        "      \"axis\": \"z\",\n      \"reversed\": false,\n      \"distribution\": {\"type\": \"extent\", \"angle\": 360},\n      \"count\": 4,",
        &format!(
            "      \"axis\": \"{rim}\",\n      \"reversed\": false,\n      \"distribution\": {{\"type\": \"spacing\", \"angle\": 45}},\n      \"count\": 2,"
        ),
    );
    let model = rebuilt(&text);
    let read = read_of(&model, "t0000002");
    let axis = read.axis.as_ref().unwrap();
    assert_eq!(axis.point, point([0.0, 0.0, 15.0]));
    let sense = axis.direction[2];
    assert_eq!(axis.direction, [0.0, 0.0, sense]);
    assert_eq!(sense.abs(), 1.0);
    assert_eq!(axis.step.map(|a| a.ndeg()), Some(45_000_000_000));
    let half = 5.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(read.instances.len(), 1);
    assert_eq!(
        read.instances[0].angle.map(|a| a.ndeg()),
        Some(45_000_000_000)
    );
    assert_eq!(
        read.instances[0].anchor,
        Some(point([half, sense * half, 0.0]))
    );
    assert_eq!(read.instances[0].state, Some(InstanceState::Built));
    // The rim's axis as `elements` describes the same edge, read another way: the same.
    let entries = silico_core::command::elements::entries(&model, &id("p0000002")).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.name.to_string() == rim)
        .unwrap_or_else(|| panic!("no {rim}"));
    let described = entry.axis.as_ref().unwrap();
    assert_eq!(described.direction, axis.direction);
    assert_eq!(described.point, axis.point);
}

#[test]
fn a_whole_solids_mirror_is_anchored_at_its_centre_of_mass() {
    // A 10 mm cube from the origin, mirrored whole in yz: its centre of mass (5, 5, 5), its
    // copy's (−5, 5, 5).
    let model = rebuilt(&pattern_docs::document(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 10.0, 10.0),
        pad("p0000001", None, "s0000001", &up("10")),
        pattern_docs::mirror("m0000001", "p0000001", pattern_docs::SOLID, "yz"),
    ]));
    let read = read_of(&model, "m0000001");
    assert_eq!(read.anchor, Some(point([5.0, 5.0, 5.0])));
    assert_eq!(read.instances[0].anchor, Some(point([-5.0, 5.0, 5.0])));
    assert_eq!(read.instances[0].state, Some(InstanceState::Built));
}

#[test]
fn copies_off_the_base_are_read_off_base_and_the_others_built_with_the_refusal() {
    // Section 4.2's example: a boss r 3 on a 40 mm block's top at x = 5, patterned along x
    // 13 apart, 5 times: copies 3 and 4, centred at 44 and 57, miss the block.
    let model = rebuilt(&pattern_docs::document(&[
        rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0),
        pad("p0000001", None, "s0000001", &up("10")),
        circles("s0000002", "xy", &[([5.0, 10.0], 3.0)]),
        pad("p0000002", Some("p0000001"), "s0000002", &up("15")),
        linear_as(
            "t0000005",
            "p0000002",
            &of("p0000002"),
            &[direction("x", "spacing", "13", 5)],
            "[]",
        ),
    ]));
    let read = read_of(&model, "t0000005");
    let off = Some(InstanceState::OffBase);
    let built = Some(InstanceState::Built);
    assert_eq!(
        states(&read),
        [
            ("1".to_owned(), built),
            ("2".to_owned(), built),
            ("3".to_owned(), off),
            ("4".to_owned(), off),
        ]
    );
    // The anchors, 13 apart from the boss's (5, 10, 0): 44 and 57 for the two off it.
    assert_eq!(read.instances[2].anchor, Some(point([44.0, 10.0, 0.0])));
    assert_eq!(read.instances[3].anchor, Some(point([57.0, 10.0, 0.0])));
    // Its problem, the refusal check gives.
    assert_eq!(read.problems_total, 1);
    assert_eq!(read.problems[0].elements(), ["3", "4"]);
}

#[test]
fn a_base_left_in_pieces_names_no_instance_so_none_has_a_state() {
    // Section 4.2's example (open decision 4(a)): a pin r 2 on a 20 × 20 plate, copied 0.001
    // along x and a kilometre along y. The base's faces lie in two solids, so the step
    // named no instance off the base: no copy has a verdict, but each is placed.
    let model = rebuilt(&pattern_docs::document(&[
        rectangle("s0000001", "xy", [-10.0, -10.0], 20.0, 20.0),
        pad("p0000011", None, "s0000001", &down("1")),
        circles("s0000002", "xy", &[([0.0, 0.0], 2.0)]),
        pad("p0000012", Some("p0000011"), "s0000002", &up("4")),
        linear_as(
            "t0000007",
            "p0000012",
            &of("p0000012"),
            &[
                direction("x", "spacing", "0.001", 2),
                direction("y", "spacing", "1000000", 2),
            ],
            "[]",
        ),
    ]));
    let read = read_of(&model, "t0000007");
    assert_eq!(
        states(&read),
        [
            ("0,1".to_owned(), None),
            ("1".to_owned(), None),
            ("1,1".to_owned(), None),
        ]
    );
    assert_eq!(
        read.instances[2].offset,
        Some(point([0.001, 1_000_000.0, 0.0]))
    );
    assert_eq!(read.problems_total, 1);
    assert!(
        read.problems[0]
            .message()
            .contains("leaves its base p0000012 in pieces"),
        "{:?}",
        read.problems[0]
    );
}

#[test]
fn a_pattern_on_a_pocket_that_failed_is_blocked_and_read_with_its_block() {
    // A hole r 2.75 pocketed at (200, 0), off the plate, removes nothing, and is refused
    // (S7); the pattern copying it is blocked on it: the read names the block, its
    // instances placed from the origin's x (an exact reference). The step's cut ran all
    // the same, its check refusing it after, and its verdict is kept: its copies, at 240
    // and 280, miss the plate too, and changed nothing.
    let model = rebuilt(&pattern_docs::document(&[
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", pattern_docs::FIVE),
        circles("s0000003", "xy", &[([200.0, 0.0], 2.75)]),
        pocket("q0000002", "p0000001", "s0000003", THROUGH),
        linear_as(
            "t0000001",
            "q0000002",
            &of("q0000002"),
            &[direction("x", "extent", "80", 3)],
            "[]",
        ),
    ]));
    assert!(matches!(
        model.outcome(&id("t0000001")),
        Some(FeatureOutcome::Blocked { .. })
    ));
    let read = read_of(&model, "t0000001");
    assert_eq!(read.anchor, Some(point([200.0, 0.0, 0.0])));
    assert_eq!(read.instances[1].anchor, Some(point([280.0, 0.0, 0.0])));
    assert_eq!(
        states(&read),
        [
            ("1".to_owned(), Some(InstanceState::ChangedNothing)),
            ("2".to_owned(), Some(InstanceState::ChangedNothing))
        ]
    );
    assert_eq!(read.problems_total, 1);
    assert_eq!(
        read.problems[0].message(),
        "t0000001 didn't build, since it uses q0000002, which failed"
    );
}

#[test]
fn a_boss_sketched_on_a_face_is_anchored_on_that_face() {
    // The plate's boss, sketched at the centre of its top face (z = 5, the global origin
    // projected onto it, F2) and padded 10, patterned 30 along x: its profile's centre is
    // (0, 0, 5) in 3D, and its copy's (30, 0, 5).
    let plate = plate_text();
    let cut = plate.find("    {\n      \"id\": \"s0000002\"").unwrap();
    let boss_at = plate.find("    {\n      \"id\": \"s0000004\"").unwrap();
    let boss_end = plate.find("    {\n      \"id\": \"s0000005\"").unwrap();
    let boss = edited(
        &plate[boss_at..boss_end],
        "\"base\": \"t0000001\"",
        "\"base\": \"p0000001\"",
    );
    let text = format!(
        "{}{}    {{\"id\": \"t0000003\", \"type\": \"linear_pattern\", \"v\": 1, \"name\": \"Bosses\", \"base\": \"p0000002\", \"copies\": {{\"type\": \"features\", \"features\": [\"p0000002\"]}}, \"directions\": [{{\"along\": \"x\", \"reversed\": false, \"distribution\": {{\"type\": \"spacing\", \"length\": 30}}, \"count\": 2}}], \"skip\": []}}\n  ]\n}}\n",
        &plate[..cut],
        boss
    );
    let model = rebuilt(&text);
    let read = read_of(&model, "t0000003");
    assert_eq!(read.anchor, Some(point([0.0, 0.0, 5.0])));
    assert_eq!(read.instances[0].anchor, Some(point([30.0, 0.0, 5.0])));
    assert_eq!(read.instances[0].state, Some(InstanceState::Built));
    assert_eq!(read.problems_total, 0, "{:?}", read.problems);
}
