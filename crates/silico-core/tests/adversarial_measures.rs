//! Hostile requests for M4.5's `distance` and `angle` (`silico_core::command::measure`) on
//! the fake kernel: names that nearly name something (case, spaces, zeros, numbers past an
//! element's kind, the solid itself, a consumed tool's face), a megabyte name, `feature`
//! naming a failed or blocked feature, two bodies, requests of every wrong shape (and the
//! snapshot agreeing), and a published read held in the kernel while the session goes on.
//! The fake places each element at (n, 0, 0), n its number, so its distances are by hand.

use super::model::WAIT;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::example::EXAMPLE;
use super::listing::{BOX, TWO_BODIES, answer, fixture, model_of};
use super::model::{checked, id, published};

use silico_core::ErrorCode;
use silico_core::command::measure::{angle, distance};
use silico_core::command::wire::measure::PairRequest;
use silico_core::command::{ModelChoice, ReadCommand};
use silico_core::document::json::read;
use silico_core::document::{ElementName, Length};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::Op;
use silico_core::recompute::Recompute;

fn pair(a: &str, b: &str) -> [String; 2] {
    [a.to_owned(), b.to_owned()]
}

fn mm(whole: i64) -> Length {
    Length::from_nm(whole * 1_000_000)
}

fn distances_asked(kernel: &FakeKernel) -> usize {
    kernel
        .calls()
        .iter()
        .filter(|c| c.op.name() == "Distance")
        .count()
}

/// The example's names refused, each as `elements.0` or `elements.1` beside a good name:
/// the name, as the refusal shows it (quoted, an invisible character by its code point, a
/// long one cut short), and what it says after it: why it isn't a name, or that it names
/// nothing.
const NEAR_MISSES: [(&str, &str, &str); 12] = [
    (
        "K7F2QX3M.box.Face5",
        r#""K7F2QX3M.box.Face5""#,
        "isn't a name: expected a feature's ID, at column 1",
    ),
    (
        "k7f2qx3m.box.face5",
        r#""k7f2qx3m.box.face5""#,
        "isn't a name: expected `Vertex`, `Edge`, `Face`, or `Solid`, at column 14",
    ),
    (
        " k7f2qx3m.box.Face5",
        r#"" k7f2qx3m.box.Face5""#,
        "isn't a name: expected a feature's ID, at column 1",
    ),
    (
        "k7f2qx3m.box.Face5 ",
        r#""k7f2qx3m.box.Face5 ""#,
        "isn't a name: more after the name, at column 19",
    ),
    (
        "k7f2qx3m.box.Face0",
        r#""k7f2qx3m.box.Face0""#,
        "isn't a name: a number from 1, without a leading zero, at column 18",
    ),
    (
        "k7f2qx3m.box.Face05",
        r#""k7f2qx3m.box.Face05""#,
        "isn't a name: a number from 1, without a leading zero, at column 18",
    ),
    (
        "k7f2qx3m.box.Face4294967296",
        r#""k7f2qx3m.box.Face4294967296""#,
        "isn't a name: a number past 4294967295, at column 18",
    ),
    (
        "",
        r#""""#,
        "isn't a name: expected a feature's ID, at column 1",
    ),
    (
        "k7f2qx3m.box.Fa\u{200b}ce5",
        r#""k7f2qx3m.box.Fa\u{200b}ce5""#,
        "isn't a name: expected `Vertex`, `Edge`, `Face`, or `Solid`, at column 14",
    ),
    (
        "k7f2qx3m.cylinder.Face1",
        r#""k7f2qx3m.cylinder.Face1""#,
        "names no element of the live solids",
    ),
    (
        "m3tq8v0z.cut.Face7",
        r#""m3tq8v0z.cut.Face7""#,
        "names no element of the live solids",
    ),
    (
        "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face4)",
        r#""m3tq8v0z.cut.Face(I a91cw0de.cylinder.Fa"…"#,
        "names no element of the live solids",
    ),
];

#[test]
fn near_miss_names_are_each_refused_where_they_are_and_nothing_reaches_the_kernel() {
    let model = model_of(EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    for (name, shown, why) in NEAR_MISSES {
        for (at, names) in [
            (0, pair(name, "k7f2qx3m.box.Face1")),
            (1, pair("k7f2qx3m.box.Face1", name)),
        ] {
            for refused in [
                distance(&model, &kernel, &names, None).unwrap_err(),
                angle(&model, &names, None).unwrap_err(),
            ] {
                assert_eq!(refused.total(), 1, "{name:?}: {:?}", refused.errors());
                let error = &refused.errors()[0];
                assert_eq!(error.code(), ErrorCode::ReferenceBroken, "{name:?}");
                let parameter = format!("elements.{at}");
                assert_eq!(error.parameter(), Some(parameter.as_str()), "{name:?}");
                assert_eq!(error.elements(), [name.to_owned()], "{name:?}");
                assert_eq!(
                    error.message(),
                    format!("{parameter}, {shown}, {why}"),
                    "{name:?}"
                );
            }
        }
    }
    assert_eq!(distances_asked(&kernel), 0);
}

#[test]
fn a_name_by_a_number_past_its_kind_suggests_the_nearest_of_that_kind_through_the_cut() {
    // The cut keeps the box's elements under the box's names: 6 faces, 12 edges, 8
    // vertices; so Face7's nearest is Face6, Edge13's Edge12, Vertex9's Vertex8, and the
    // last number a name holds, 4294967295, Face6 too. The pin has three faces, each made a
    // face of the cut: its Face4's nearest is what the cut made of its Face3; its Face1,
    // consumed, by its own name, suggests what the cut made of it.
    let model = model_of(EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    for (broken, candidate) in [
        ("k7f2qx3m.box.Face7", "k7f2qx3m.box.Face6"),
        ("k7f2qx3m.box.Edge13", "k7f2qx3m.box.Edge12"),
        ("k7f2qx3m.box.Vertex9", "k7f2qx3m.box.Vertex8"),
        ("k7f2qx3m.box.Face4294967295", "k7f2qx3m.box.Face6"),
        (
            "a91cw0de.cylinder.Face4",
            "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face3)",
        ),
        (
            "a91cw0de.cylinder.Face1",
            "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)",
        ),
        ("k7f2qx3m.box.Face5~1", "k7f2qx3m.box.Face5"),
    ] {
        let refused =
            distance(&model, &kernel, &pair("k7f2qx3m.box.Face1", broken), None).unwrap_err();
        assert_eq!(refused.total(), 1, "{broken}");
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::ReferenceBroken, "{broken}");
        assert_eq!(error.parameter(), Some("elements.1"), "{broken}");
        assert_eq!(error.candidates(), [candidate.to_owned()], "{broken}");
    }
    // The consumed pin's face, with the pin as the feature: found, Face1 to Face3, 2 apart.
    let pin = distance(
        &model,
        &kernel,
        &pair("a91cw0de.cylinder.Face1", "a91cw0de.cylinder.Face3"),
        Some(&id("a91cw0de")),
    )
    .unwrap();
    assert_eq!(pin.distance, mm(2));
}

#[test]
fn a_refusal_that_gives_no_candidates_doesnt_say_it_does() {
    // Fixed: a refusal with no candidates suggests `elements`, not candidates it hasn't; was: every name refused, whatever its reason, carried the suggestion "name the
    // element as the model now names it: the candidates are the likeliest, none taken for
    // it", candidates or none: a name that isn't one (`K7F2QX3M.box.Face5`) has none, and
    // a box's element of a cylinder's maker none either. An agent is pointed at an empty
    // list. Where there are none, the suggestion should say what to do instead (list the
    // solid's elements with `elements`, or mend the name as the cause says).
    let model = model_of(EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    for name in ["K7F2QX3M.box.Face5", "k7f2qx3m.cylinder.Face1"] {
        let refused =
            distance(&model, &kernel, &pair(name, "k7f2qx3m.box.Face1"), None).unwrap_err();
        let error = &refused.errors()[0];
        assert!(error.candidates().is_empty(), "{name}");
        let suggestion = error.suggestion().unwrap_or_default();
        assert!(
            !suggestion.contains("candidates"),
            "{name}: {suggestion:?} with no candidates"
        );
    }
}

#[test]
fn a_solids_own_name_is_refused_as_a_solid() {
    // A lone box's solid, `bbbbbbbb.box.Solid1`: the design refuses a name of a solid ("a
    // measure is between faces, edges, and vertices"), and `find` has that refusal.
    // Fixed: a solid's name is refused as one as soon as it's read; was: element maps name no solids, so the name found nothing, and was refused as
    // `reference_broken`, "names no element of the live solids", no candidates, told to
    // name the element as the model now names it; the solid refusal is never reached.
    // It should be `invalid_input`, naming elements.0, saying it names a solid.
    let model = model_of(BOX);
    let kernel = FakeKernel::new(1).unwrap();
    let refused = distance(
        &model,
        &kernel,
        &pair("bbbbbbbb.box.Solid1", "bbbbbbbb.box.Face1"),
        None,
    )
    .unwrap_err();
    assert_eq!(refused.total(), 1);
    let error = &refused.errors()[0];
    assert_eq!(error.parameter(), Some("elements.0"));
    assert_eq!(
        (error.code(), error.message()),
        (
            ErrorCode::InvalidInput,
            r#"elements.0, "bbbbbbbb.box.Solid1", names a solid: a measure is between faces, edges, and vertices"#
        )
    );
}

#[test]
fn a_megabyte_name_is_refused_at_once_and_shown_short() {
    let model = model_of(EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    for name in [
        format!("k7f2qx3m.box.Face5{}", "x".repeat(1 << 20)),
        format!("k7f2qx3m.box.Face{}", "9".repeat(1 << 20)),
        format!(
            "m3tq8v0z.cut.Face({})",
            "I k7f2qx3m.box.Face1, ".repeat(40_000)
        ),
    ] {
        let started = Instant::now();
        let refused = distance(
            &model,
            &kernel,
            &[name.clone(), "k7f2qx3m.box.Face1".to_owned()],
            None,
        )
        .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(refused.total(), 1);
        let error = &refused.errors()[0];
        assert_eq!(error.code(), ErrorCode::ReferenceBroken);
        assert!(error.message().len() < 400, "{}", error.message().len());
        assert!(
            error.elements()[0].len() < 300,
            "{}",
            error.elements()[0].len()
        );
        assert!(error.candidates().iter().all(|c| c.len() < 300));
    }
    assert_eq!(distances_asked(&kernel), 0);
}

/// The example, and a fillet of the cut by an edge it hasn't (broken), and a fillet of that
/// fillet (blocked on it).
fn with_failures() -> String {
    EXAMPLE.replace(
        r#""note": "Clearance for an M5 bolt"}"#,
        r#""note": "Clearance for an M5 bolt"},
    {"id": "ffffffff", "type": "fillet", "v": 1, "name": "Broken", "base": "m3tq8v0z", "edges": ["k7f2qx3m.box.Edge13"], "radius": 1},
    {"id": "gggggggg", "type": "fillet", "v": 1, "name": "Blocked", "base": "ffffffff", "edges": ["k7f2qx3m.box.Edge1"], "radius": 1}"#,
    )
}

#[test]
fn feature_naming_a_failed_or_blocked_feature_says_so_and_the_cut_stays_a_body() {
    let text = with_failures();
    assert!(text.contains("gggggggg"));
    let model = model_of(&text);
    let kernel = FakeKernel::new(1).unwrap();
    let names = pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6");
    // The broken fillet: its own broken reference, naming it.
    let broken = distance(&model, &kernel, &names, Some(&id("ffffffff"))).unwrap_err();
    assert_eq!(broken.total(), 1, "{:?}", broken.errors());
    assert_eq!(broken.errors()[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(broken.errors()[0].feature(), Some("ffffffff"));
    // The blocked one: what it waits on, naming feature.
    let blocked = angle(&model, &names, Some(&id("gggggggg"))).unwrap_err();
    assert_eq!(blocked.total(), 1, "{:?}", blocked.errors());
    let error = &blocked.errors()[0];
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.parameter(), Some("feature"));
    assert_eq!(
        error.message(),
        "gggggggg didn't build, since it uses ffffffff, which failed"
    );
    assert_eq!(distances_asked(&kernel), 0);
    // With no feature: a failed feature consumes nothing, so the cut is still a body and
    // its faces are found, 5 and 6, 1 apart.
    assert_eq!(
        distance(&model, &kernel, &names, None).unwrap().distance,
        mm(1)
    );
}

#[test]
fn elements_of_two_bodies_send_both_solids_and_one_bodys_only_its_own() {
    // On the fake, Face1 of the first box and Face4 of the second are 3 apart; the
    // second's solid is sent. Two of the first's vertices, 1 and 8, are 7 apart, centre to
    // centre too (each vertex its own centre); one solid sent.
    let model = model_of(TWO_BODIES);
    let kernel = FakeKernel::new(1).unwrap();
    let across = distance(
        &model,
        &kernel,
        &pair("aaaaaaaa.box.Face1", "bbbbbbbb.box.Face4"),
        None,
    )
    .unwrap();
    assert_eq!(across.distance, mm(3));
    let within = distance(
        &model,
        &kernel,
        &pair("aaaaaaaa.box.Vertex1", "aaaaaaaa.box.Vertex8"),
        None,
    )
    .unwrap();
    assert_eq!(within.distance, mm(7));
    assert_eq!(within.center_distance, Some(mm(7)));
    assert_eq!(within.normal_distance, None);
    let others: Vec<bool> = kernel
        .calls()
        .iter()
        .filter_map(|c| match &c.op {
            Op::Distance { other, .. } => Some(other.is_some()),
            _ => None,
        })
        .collect();
    assert_eq!(others, [true, false]);
    // Both broken, each in another body's terms: both said, in order, one error each.
    let refused = distance(
        &model,
        &kernel,
        &pair("aaaaaaaa.box.Face9", "bbbbbbbb.box.Edge13"),
        None,
    )
    .unwrap_err();
    let said: Vec<(Option<&str>, Vec<String>)> = refused
        .errors()
        .iter()
        .map(|e| (e.parameter(), e.candidates().to_vec()))
        .collect();
    assert_eq!(
        said,
        [
            (Some("elements.0"), vec!["aaaaaaaa.box.Face6".to_owned()]),
            (Some("elements.1"), vec!["bbbbbbbb.box.Edge12".to_owned()]),
        ]
    );
}

fn request(elements: &str) -> String {
    format!(r#"{{"session": "s1", "model": "current", "elements": {elements}, "feature": null}}"#)
}

/// A problem's parameter and message.
type Said = (Option<String>, String);

/// What `text` is refused for, each problem's parameter and message.
fn refusals(text: &str) -> Vec<Said> {
    PairRequest::read(&read(text.as_bytes()).unwrap())
        .unwrap_err()
        .errors()
        .iter()
        .map(|e| (e.parameter().map(str::to_owned), e.message().to_owned()))
        .collect()
}

fn said(parameter: &str, message: &str) -> Said {
    (Some(parameter.to_owned()), message.to_owned())
}

/// The committed snapshot's request schema for `of`.
fn snapshot(of: &str) -> jsonschema::Validator {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/contract/commands/{of}.json"));
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    jsonschema::validator_for(&schema["request"]).unwrap()
}

#[test]
fn a_pair_request_of_every_wrong_shape_is_refused_by_its_place_as_the_snapshot_refuses_it() {
    let schemas = [snapshot("distance"), snapshot("angle")];
    let cases: [(&str, Vec<Said>); 10] = [
        (
            r#"["a", "b", "c"]"#,
            vec![said(
                "elements",
                "elements must be a list of 2 names, got 3 of them",
            )],
        ),
        (
            "[]",
            vec![said(
                "elements",
                "elements must be a list of 2 names, got 0 of them",
            )],
        ),
        (
            "3",
            vec![said(
                "elements",
                "elements must be a list of 2 names, got 3",
            )],
        ),
        (
            "null",
            vec![said(
                "elements",
                "elements must be a list of 2 names, got null",
            )],
        ),
        (
            r#""a""#,
            vec![said(
                "elements",
                r#"elements must be a list of 2 names, got "a""#,
            )],
        ),
        (
            r#"{"0": "a", "1": "b"}"#,
            vec![said(
                "elements",
                "elements must be a list of 2 names, got an object",
            )],
        ),
        (
            r#"[["a"], ["b"]]"#,
            vec![
                said("elements.0", "elements.0 must be a name, got an array"),
                said("elements.1", "elements.1 must be a name, got an array"),
            ],
        ),
        (
            "[null, 1.5]",
            vec![
                said("elements.0", "elements.0 must be a name, got null"),
                said("elements.1", "elements.1 must be a name, got 1.5"),
            ],
        ),
        (
            r#"["a", {"name": "b"}]"#,
            vec![said(
                "elements.1",
                "elements.1 must be a name, got an object",
            )],
        ),
        (
            r#"["a", "b", 3]"#,
            vec![said(
                "elements",
                "elements must be a list of 2 names, got 3 of them",
            )],
        ),
    ];
    for (elements, expected) in cases {
        let text = request(elements);
        assert_eq!(refusals(&text), expected, "{elements}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for schema in &schemas {
            assert!(!schema.is_valid(&value), "the snapshot takes {elements}");
        }
    }
    // Strings that spell a literal or a number are names to the reader (and refused later,
    // by the command, as names that aren't), as they are to the snapshot.
    let text = request(r#"["null", "3"]"#);
    let read = PairRequest::read(&read(text.as_bytes()).unwrap()).unwrap();
    assert_eq!(read.elements, pair("null", "3"));
    for schema in &schemas {
        assert!(schema.is_valid(&serde_json::from_str(&text).unwrap()));
    }
}

#[test]
fn a_million_names_are_refused_by_their_count_at_once() {
    let text = request(&format!(
        "[{}]",
        vec![r#""k7f2qx3m.box.Face1""#; 1_000_000].join(",")
    ));
    let started = Instant::now();
    let refused = refusals(&text);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        refused,
        [said(
            "elements",
            "elements must be a list of 2 names, got 1000000 of them"
        )]
    );
}

#[test]
fn a_pair_request_wrong_every_way_says_every_way_in_order() {
    let text = r#"{"session": 5, "model": "x", "elements": 3, "feature": 7, "zzz": 1}"#;
    assert_eq!(
        refusals(text),
        [
            said(
                "session",
                "session must be a session's ID, s1, s2, ..., got 5"
            ),
            // A plain word refused is said bare, as the wire says every word (`got solid`).
            said("model", "model must be current or published, got x"),
            said("elements", "elements must be a list of 2 names, got 3"),
            said("feature", "feature must be a feature's ID or null, got 7"),
            said("zzz", "zzz isn't a field of this request"),
        ]
    );
    // Each field required, `feature` too (null, not left out).
    assert_eq!(
        refusals(r#"{"session": "s1", "model": "current", "elements": ["a", "b"]}"#),
        [said("feature", "feature is missing")]
    );
    assert_eq!(
        refusals(r#"{"session": "s1", "model": "current", "feature": null}"#),
        [said("elements", "elements is missing")]
    );
}

#[test]
fn a_published_distance_held_in_the_kernel_holds_up_nothing_else_of_the_session() {
    let f = fixture();
    let s = f.open("part.silico", EXAMPLE);
    let faces = || pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6");
    // Nothing published yet: refused, naming model, nothing asked.
    let none = answer(
        f.sessions
            .distance(&s, ModelChoice::Published, faces(), None),
    );
    let problems = none.result.unwrap_err();
    assert_eq!(problems.errors()[0].parameter(), Some("model"));
    assert_eq!(distances_asked(&f.kernel), 0);
    // Built and published, then a published distance held in the kernel.
    assert!(
        answer(
            f.sessions
                .elements(&s, ModelChoice::Current, None, None, 10)
        )
        .result
        .is_ok()
    );
    let hold = f.kernel.hold(|op| op.name() == "Distance");
    let held = f
        .sessions
        .distance(&s, ModelChoice::Published, faces(), None);
    let held = match held.wait_timeout(Duration::from_millis(200)) {
        Ok(response) => panic!("answered while held: {:?}", response.result),
        Err(running) => running,
    };
    // The session goes on: reads of either model, and closing it, answer at once.
    for model in [ModelChoice::Current, ModelChoice::Published] {
        let listed = f
            .sessions
            .elements(&s, model, None, None, 10)
            .wait_timeout(WAIT);
        assert!(listed.is_ok_and(|r| r.result.is_ok()), "{model:?}");
    }
    let closed = f.sessions.close(&s).wait_timeout(WAIT);
    assert!(closed.is_ok_and(|r| r.result.is_ok()));
    // Closing it retired the read's kernel client (design §2: a read's client ends when
    // its session closes; from the measure council's architecture review), so the read
    // answers, without its hold being released, that the session closed.
    let response = held.wait_timeout(WAIT).ok().unwrap();
    let refused = response.result.unwrap_err();
    assert_eq!(
        refused.errors()[0].message(),
        "the session closed before the kernel measured the distance"
    );
    hold.release();
}

#[test]
fn an_undescribed_body_is_refused_once_and_blames_no_feature_the_request_didnt_name() {
    // A box whose description the kernel refuses: it builds, but its elements' geometry
    // isn't known, so neither measure can be had, and nothing is asked of the kernel.
    let kernel = Arc::new(FakeKernel::new(1).unwrap());
    kernel.refuse(|op| {
        (op.name() == "Describe")
            .then(|| silico_core::Error::new(ErrorCode::KernelFailed, "no description"))
    });
    let engine = Recompute::new(Arc::clone(&kernel)).with_sketches(FakeSketches::new());
    let model = published(&engine, checked(BOX));
    let faces = pair("bbbbbbbb.box.Face1", "bbbbbbbb.box.Face2");
    // Fixed: the solid's failure is said once, naming `feature` only if the request did; was: said once for each element (twice here, both in the
    // box), whether `feature` names it or not; and with `feature` null it still names
    // `feature`, a parameter the request left null, so an agent is sent to mend a field
    // it never set. `find` reads the failure through `elements::entries`, which always
    // names `feature`, once for each element; it should say each solid's failure once,
    // naming `feature` only when the request did (`solid(model, id, feature.is_some())`).
    let said = |refused: silico_core::command::Problems| -> Vec<(ErrorCode, Option<String>, Option<String>)> {
        refused
            .errors()
            .iter()
            .map(|e| (e.code(), e.feature().map(str::to_owned), e.parameter().map(str::to_owned)))
            .collect()
    };
    let failed = |parameter: Option<&str>| {
        vec![(
            ErrorCode::KernelFailed,
            Some("bbbbbbbb".to_owned()),
            parameter.map(str::to_owned),
        )]
    };
    let block = Some(id("bbbbbbbb"));
    for (feature, parameter) in [(None, None), (block.as_ref(), Some("feature"))] {
        assert_eq!(
            said(distance(&model, &*kernel, &faces, feature).unwrap_err()),
            failed(parameter),
            "{feature:?}"
        );
        assert_eq!(
            said(angle(&model, &faces, feature).unwrap_err()),
            failed(parameter),
            "{feature:?}"
        );
    }
    assert_eq!(distances_asked(&kernel), 0);
}

// ---- From the measure council (round 2) ----

#[test]
fn a_failed_feature_asked_for_with_its_own_elements_is_said_once() {
    // The request's feature failed, and both names are of its own elements: its failure,
    // once, not again for each name.
    let model = model_of(&with_failures());
    let kernel = FakeKernel::new(1).unwrap();
    let names = pair(
        "ffffffff.fillet.Face(G k7f2qx3m.box.Edge1)",
        "ffffffff.fillet.Face(G k7f2qx3m.box.Edge2)",
    );
    let refused = distance(&model, &kernel, &names, Some(&id("ffffffff"))).unwrap_err();
    assert_eq!(refused.total(), 1, "{:?}", refused.errors());
    // And with no feature, each name's maker said once for both.
    let refused = distance(&model, &kernel, &names, None).unwrap_err();
    assert_eq!(refused.total(), 1, "{:?}", refused.errors());
}

#[test]
fn a_published_read_that_asks_the_kernel_after_its_session_closes_says_it_closed() {
    // The read waits inside its closure while the session closes, then asks: its client was
    // retired, so the request, made after, is superseded too, and the kernel asked nothing.
    let f = fixture();
    let s = f.open("part.silico", EXAMPLE);
    assert!(
        answer(
            f.sessions
                .elements(&s, ModelChoice::Current, None, None, 10)
        )
        .result
        .is_ok()
    );
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let read = f.sessions.read_model(
        &s,
        ReadCommand::Distance,
        ModelChoice::Published,
        move |read| {
            let _ = wait.recv();
            distance(
                read.model(),
                read.kernel(),
                &pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
                None,
            )
        },
    );
    let read = match read.wait_timeout(Duration::from_millis(200)) {
        Ok(response) => panic!("answered early: {:?}", response.result),
        Err(running) => running,
    };
    assert!(
        f.sessions
            .close(&s)
            .wait_timeout(WAIT)
            .is_ok_and(|r| r.result.is_ok())
    );
    go.send(()).unwrap();
    let refused = read.wait_timeout(WAIT).ok().unwrap().result.unwrap_err();
    assert_eq!(
        refused.errors()[0].message(),
        "the session closed before the kernel measured the distance"
    );
    assert_eq!(distances_asked(&f.kernel), 0);
}

#[test]
fn a_held_read_released_before_its_session_closes_answers_its_distance() {
    // Held in the kernel, it's still running while other reads come and go; released, it
    // answers the model it read: Face5 to Face6, 1.
    let f = fixture();
    let s = f.open("part.silico", EXAMPLE);
    assert!(
        answer(
            f.sessions
                .elements(&s, ModelChoice::Current, None, None, 10)
        )
        .result
        .is_ok()
    );
    let hold = f.kernel.hold(|op| op.name() == "Distance");
    let held = f.sessions.distance(
        &s,
        ModelChoice::Published,
        pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    );
    for model in [ModelChoice::Current, ModelChoice::Published] {
        assert!(
            f.sessions
                .elements(&s, model, None, None, 10)
                .wait_timeout(WAIT)
                .is_ok_and(|r| r.result.is_ok())
        );
    }
    let held = match held.wait_timeout(Duration::from_millis(200)) {
        Ok(response) => panic!("not held: {:?}", response.result),
        Err(running) => running,
    };
    hold.release();
    let response = held.wait_timeout(WAIT).ok().unwrap();
    assert_eq!(response.result.unwrap().distance, mm(1));
}

#[test]
fn dropping_the_sessions_stops_a_current_read_held_in_the_kernel() {
    // A current read runs on its session's own thread; dropping the sessions closes it,
    // retiring the read's client, so the drop doesn't wait for the held request.
    let f = fixture();
    let s = f.open("part.silico", EXAMPLE);
    assert!(
        answer(
            f.sessions
                .elements(&s, ModelChoice::Current, None, None, 10)
        )
        .result
        .is_ok()
    );
    let hold = f.kernel.hold(|op| op.name() == "Distance");
    let reading = f.sessions.distance(
        &s,
        ModelChoice::Current,
        pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    );
    assert!(f.kernel.wait_running(1, WAIT));
    let started = Instant::now();
    drop(f.sessions);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let refused = reading.wait_timeout(WAIT).ok().unwrap().result.unwrap_err();
    assert_eq!(
        refused.errors()[0].message(),
        "the session closed before the kernel measured the distance"
    );
    hold.release();
}

#[test]
fn a_worker_that_crashed_isnt_the_requests_fault_but_a_refused_value_is() {
    let model = model_of(EXAMPLE);
    let names = pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6");
    for (code, theirs) in [
        (ErrorCode::WorkerCrashed, false),
        (ErrorCode::WorkerTimeout, false),
        (ErrorCode::InvalidResult, true),
        (ErrorCode::KernelFailed, true),
    ] {
        let kernel = FakeKernel::new(1).unwrap();
        kernel.refuse(move |op| {
            (op.name() == "Distance").then(|| silico_core::Error::new(code, "as the test has it"))
        });
        let refused = distance(&model, &kernel, &names, None).unwrap_err();
        let error = &refused.errors()[0];
        assert_eq!(error.code(), code);
        assert_eq!(
            error.elements(),
            ["k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"],
            "{code:?}"
        );
        assert_eq!(error.parameter(), theirs.then_some("elements"), "{code:?}");
    }
}

// ---- From the measure council (round 3) ----

/// A box `aaaaaaaa` cut by `bbbbbbbb` (`rrrrrrrr`), and that cut fused with a third box,
/// `cccccccc` (`ffffffff`): the one body.
const CUT_THEN_FUSED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "First", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Second", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "rrrrrrrr", "type": "cut", "v": 1, "name": "Cut", "base": "aaaaaaaa", "tool": "bbbbbbbb"}, {"id": "cccccccc", "type": "box", "v": 1, "name": "Third", "position": [9, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "ffffffff", "type": "fuse", "v": 1, "name": "Both", "base": "rrrrrrrr", "tool": "cccccccc"}]}"#;

#[test]
fn the_vertex_two_edges_meet_at_is_named_by_its_own_name_though_unstable() {
    // Found by the tests review: `angle` names the vertex two straight edges share by its
    // own name (its first), as `elements` lists it, even when that name is unstable; the
    // code it replaced took the first stable name, falling back to the own name, and no
    // test told the two apart, since every test vertex's own name was stable. Here one
    // isn't, and it has a stable alias. The cut keeps all of `aaaaaaaa` but its Vertex1,
    // and nothing of the tool, so its Vertex1 is named by its number, `rrrrrrrr.cut.Vertex1`,
    // unstable (N3). The fuse keeps all of the cut, and the third box's Vertex1 as its
    // Vertex1 too: that vertex's names are the cut's (its own, unstable, kept so) and then
    // `cccccccc.box.Vertex1` (stable). Edge2 and Edge3, kept from `aaaaaaaa` throughout, are
    // forged straight: both from the fake's Vertex1 point, (1, 0, 0), one to (2, 0, 0) and
    // one to (0, 1, 0): directions (1, 0, 0) and (-1, 1, 0), 135 degrees apart at the vertex
    // (45 between the lines, which a lost shared vertex would read).
    use silico_core::command::elements::entries;
    use silico_core::document::Angle;
    use silico_core::kernel::{
        BooleanOp, BuiltData, ElementCounts, ElementFate, ElementKind, History, InputHistory,
        Topology,
    };
    use silico_protocol::{Description, EdgeCurve, EdgeDescription, Reply};

    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    let none = |n: usize| vec![ElementFate::default(); n];
    let all = |n: u32| (1..=n).map(kept).collect::<Vec<_>>();
    let box_counts = ElementCounts {
        faces: 6,
        edges: 12,
        vertices: 8,
        solids: 1,
    };
    let topology = Topology {
        face_edges: vec![vec![]; 6],
        outer_edges: vec![vec![]; 6],
        edge_vertices: vec![vec![]; 12],
    };
    let cut = History {
        inputs: vec![
            InputHistory {
                faces: all(6),
                edges: all(12),
                vertices: none(1).into_iter().chain((2..=8).map(kept)).collect(),
            },
            InputHistory {
                faces: none(6),
                edges: none(12),
                vertices: none(8),
            },
        ],
        result: box_counts,
        ends: Default::default(),
        topology: topology.clone(),
    };
    let fuse = History {
        inputs: vec![
            InputHistory {
                faces: all(6),
                edges: all(12),
                vertices: all(8),
            },
            InputHistory {
                faces: none(6),
                edges: none(12),
                vertices: std::iter::once(kept(1)).chain(none(7)).collect(),
            },
        ],
        result: box_counts,
        ends: Default::default(),
        topology,
    };
    cut.validate(2).unwrap();
    fuse.validate(2).unwrap();
    let straight = |start: [f64; 3], end: [f64; 3]| EdgeDescription {
        curve: EdgeCurve::Line,
        length: libm::hypot(end[0] - start[0], end[1] - start[1]),
        start,
        end,
        middle: [(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0, 0.0],
    };
    // Every other edge as the fake has it: of type other, from (n, 0, 0) to (n, 1, 0).
    let edges: Vec<EdgeDescription> = (1..=12u32)
        .map(|n| match n {
            2 => straight([1.0, 0.0, 0.0], [2.0, 0.0, 0.0]),
            3 => straight([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            n => {
                let x = f64::from(n);
                EdgeDescription {
                    curve: EdgeCurve::Other,
                    length: 1.0,
                    start: [x, 0.0, 0.0],
                    end: [x, 1.0, 0.0],
                    middle: [x, 0.5, 0.0],
                }
            }
        })
        .collect();
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().answer_instead(move |op| match op {
        Op::Boolean {
            op: BooleanOp::Cut,
            base,
            ..
        } => Some(Reply::Built(BuiltData {
            shape: base.clone(),
            history: cut.clone(),
        })),
        Op::Boolean {
            op: BooleanOp::Fuse,
            base,
            ..
        } => Some(Reply::Built(BuiltData {
            shape: base.clone(),
            history: fuse.clone(),
        })),
        // Every element at once: the edges given, the faces and vertices the fake's.
        Op::DescribeAll { shape } => {
            let fake = FakeKernel::described(shape)?;
            Some(Reply::DescribedAll(Description {
                faces: fake.faces,
                edges: edges.clone(),
                vertices: fake.vertices,
            }))
        }
        Op::Describe {
            kind: ElementKind::Edge,
            first,
            count,
            ..
        } => {
            let from = (*first - 1) as usize;
            let to = (from + *count as usize).min(edges.len());
            Some(Reply::Described {
                total: edges.len() as u32,
                description: Description {
                    faces: Vec::new(),
                    edges: edges[from..to].to_vec(),
                    vertices: Vec::new(),
                },
            })
        }
        _ => None,
    });
    let model = published(&engine, checked(CUT_THEN_FUSED));
    // The vertex's entry: its own name unstable, its alias stable.
    let listed = entries(&model, &id("ffffffff")).unwrap();
    let entry = listed
        .iter()
        .find(|e| e.kind == ElementKind::Vertex && e.name.to_string() == "rrrrrrrr.cut.Vertex1")
        .unwrap_or_else(|| {
            panic!(
                "no rrrrrrrr.cut.Vertex1: {:?}",
                listed
                    .iter()
                    .map(|e| e.name.to_string())
                    .collect::<Vec<_>>()
            )
        });
    assert!(!entry.stable);
    let aliases: Vec<(String, bool)> = entry
        .aliases
        .iter()
        .map(|a| (a.name.to_string(), a.stable))
        .collect();
    assert_eq!(aliases, [("cccccccc.box.Vertex1".to_owned(), true)]);
    for (names, feature) in [
        (pair("aaaaaaaa.box.Edge2", "aaaaaaaa.box.Edge3"), None),
        (
            pair("aaaaaaaa.box.Edge3", "aaaaaaaa.box.Edge2"),
            Some(id("ffffffff")),
        ),
    ] {
        let answer = angle(&model, &names, feature.as_ref()).unwrap();
        assert_eq!(
            answer.vertex.as_ref().map(|v| v.as_str()),
            Some("rrrrrrrr.cut.Vertex1"),
            "{names:?}"
        );
        assert_eq!(
            answer.vertex,
            Some(ElementName::from(&*entry.name)),
            "{names:?}"
        );
        assert_eq!(answer.angle, Angle::from_ndeg(135_000_000_000), "{names:?}");
    }
}

/// A triangle sketch, padded.
const PADDED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 20, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}]}"#;

#[test]
fn a_name_of_a_sketchs_element_is_broken_where_it_is_not_the_sketchs_no_solid() {
    // The robustness review's worry: a name of a sketch's own element (its line
    // `s0000001.sketch.Edge.na000000`, which the pad's side face's name contains, so an
    // agent may well try it) answered with the sketch's own refusal, that it's a sketch and
    // makes no solid, with no `elements.N`, as though the request had named the sketch as
    // its `feature`. It isn't: the name reader refuses it first (a sketch's element, whole,
    // is no name a reference takes: S4), broken where it is, whether `feature` is the pad
    // or none, and in either place. This holds that refusal, of a sketch's curve, point,
    // region, and a `~n` of one alike.
    let model = model_of(PADDED);
    let kernel = FakeKernel::new(1).unwrap();
    let face = "p0000001.pad.Face(G s0000001.sketch.Face1)";
    let pad = id("p0000001");
    for sketched in [
        "s0000001.sketch.Edge.na000000",
        "s0000001.sketch.Vertex.pa000000",
        "s0000001.sketch.Face1",
        "s0000001.sketch.Edge.na000000~1",
    ] {
        for feature in [None, Some(&pad)] {
            for (at, names) in [(0, pair(sketched, face)), (1, pair(face, sketched))] {
                let case = format!("{sketched}, {feature:?}, elements.{at}");
                let refused = distance(&model, &kernel, &names, feature).unwrap_err();
                assert_eq!(refused.total(), 1, "{case}: {:?}", refused.errors());
                let error = &refused.errors()[0];
                assert_eq!(error.code(), ErrorCode::ReferenceBroken, "{case}");
                let parameter = format!("elements.{at}");
                assert_eq!(error.parameter(), Some(parameter.as_str()), "{case}");
                assert_eq!(error.elements(), [sketched.to_owned()], "{case}");
                assert_eq!(error.feature(), None, "{case}");
                assert_eq!(
                    error.message(),
                    format!(
                        "{parameter}, \"{sketched}\", isn't a name: s0000001 is a sketch: its \
                         elements are in no solid; name what a feature made of it, at column 1"
                    ),
                    "{case}"
                );
            }
        }
    }
    assert_eq!(distances_asked(&kernel), 0);
}
