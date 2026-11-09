//! A broken name's candidates' `ranking` (Fable's decision of 2026-11-08, for the
//! maintainer), over the sweep features' maps `sweep_names` builds: names that find nothing,
//! made from the maps' own names, read as a selection reads them (`selection::resolve_in`),
//! and each candidate's rank held to what the names themselves say. The elements a name is
//! made from are read off its text here, never by the core's own walk: each `id.type.KindN`
//! (an element by its number) and each `id.sketch.Kind.id` (a sketch's by its ID) in it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model::WAIT;
use super::sweeps;

use std::collections::BTreeSet;
use std::sync::Arc;

use silico_core::document::FeatureId;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::naming::{Maker, Name};
use silico_core::recompute::{Model, Recompute, RunOutcome};
use silico_core::{ErrorCode, Rank};
use sweeps::*;

fn published(features: &[&str]) -> Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    match engine.start(checked_document(features)).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("not published: {:?}", other.ok()),
    }
}

/// Whether `token` is an element by its number: an ID, a type word, and a kind with its
/// number (`k7f2qx3m.box.Face6`).
fn by_number(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    let [id, word, element] = parts[..] else {
        return false;
    };
    let digits = element.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    id.len() == 8
        && !word.is_empty()
        && ["Face", "Edge", "Vertex"].contains(&&element[..element.len() - digits.len()])
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `token` is a sketch's element by its ID (`s0000001.sketch.Edge.na000001`).
fn by_id(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    matches!(parts[..], [id, "sketch", "Edge" | "Vertex", member] if id.len() == 8 && member.len() == 8)
}

/// The elements the name `text` is made from, read off its text: every token between its
/// brackets, commas, and spaces that's an element by its number or a sketch's by its ID.
fn leaves(text: &str) -> BTreeSet<String> {
    text.split([' ', '(', ')', ','])
        .filter(|token| by_number(token) || by_id(token))
        .map(str::to_owned)
        .collect()
}

/// Where `leaf` sorts among the elements a name is made from, as `Ord for Name` sorts them,
/// read off its text: elements by their number before a sketch's by ID; then by feature;
/// then by kind, vertex, edge, face, as `ElementKind` lists them; then by number, or by ID.
fn order(leaf: &str) -> (bool, String, u8, u64, String) {
    let kind = |word: &str| match word {
        "Vertex" => 0,
        "Edge" => 1,
        _ => 2,
    };
    let parts: Vec<&str> = leaf.split('.').collect();
    if by_id(leaf) {
        return (
            true,
            parts[0].to_owned(),
            kind(parts[2]),
            0,
            parts[3].to_owned(),
        );
    }
    let (owner, number) = numbered(leaf);
    let word = owner.rsplit('.').next().unwrap();
    (
        false,
        parts[0].to_owned(),
        kind(word),
        number,
        String::new(),
    )
}

/// An element by its number's kind and owner (`k7f2qx3m.box.Face`) and its number.
fn numbered(token: &str) -> (&str, u64) {
    let digits = token.trim_end_matches(|c: char| c.is_ascii_digit());
    (digits, token[digits.len()..].parse().unwrap())
}

/// Broken names made from `names`, those of a solid's map: each name with one of the
/// elements it's made from swapped for another of the map's (what a reference to an element
/// an edit remade looks like), and each element by its number past its feature's last (a
/// box's Face9); none that finds an element, or that doesn't read as a name.
fn broken_from(names: &BTreeSet<String>) -> Vec<String> {
    let all: BTreeSet<String> = names.iter().flat_map(|name| leaves(name)).collect();
    let mut broken = BTreeSet::new();
    for name in names {
        for leaf in leaves(name) {
            for other in all.iter().filter(|other| **other != leaf) {
                broken.insert(name.replacen(&leaf, other, 1));
            }
        }
    }
    for leaf in all.iter().filter(|leaf| by_number(leaf)) {
        let (owner, number) = numbered(leaf);
        for past in 1..=8 {
            broken.insert(format!("{owner}{}", number + past));
        }
    }
    broken.retain(|name| !names.contains(name));
    broken.into_iter().collect()
}

/// What the property found, to show it's tested something of both rules.
#[derive(Default, Debug)]
struct Seen {
    alike: usize,
    nearest: usize,
    other_feature: usize,
    some_unshared: usize,
}

/// Every broken name made from `feature`'s map in `model`, read against that solid: each
/// one with candidates ranked as its names say.
fn holds(model: &Model, feature: &str, seen: &mut Seen) {
    let makers = |id: &FeatureId| {
        model
            .document()
            .features
            .iter()
            .find(|f| &f.id == id)
            .map(|f| Maker::of(&f.operation))
    };
    let map = model.element_map(&id(feature)).unwrap();
    let names: BTreeSet<String> = map
        .iter()
        .map(|(_, names)| names.first().unwrap().to_string())
        .collect();
    let broken = broken_from(&names);
    let read = |text: &str| Name::parse(text, &makers).unwrap_or_else(|e| panic!("{text}: {e}"));
    let resolved =
        silico_core::selection::resolve_in(model, &broken, "selection", &id(feature)).unwrap();
    assert!(resolved.pending.is_empty(), "{:?}", resolved.pending);
    for gone in &resolved.gone {
        let error = &gone.why.errors()[0];
        if error.code() != ErrorCode::ReferenceBroken || error.candidates().is_empty() {
            assert!(error.ranking().is_empty(), "{}: {error}", gone.name);
            continue;
        }
        let (name, candidates, ranking) = (&gone.name, error.candidates(), error.ranking());
        assert_eq!(ranking.len(), candidates.len(), "{name}");
        assert!(candidates.len() <= 5, "{name}");
        let own = leaves(name);
        let alike = ranking.iter().any(|rank| !rank.shared.is_empty());
        for (candidate, rank) in candidates.iter().zip(ranking) {
            let theirs = leaves(candidate);
            let Rank {
                shared,
                unshared,
                same_feature,
                nearest,
            } = rank;
            // `shared` and `unshared` part the broken name's elements: each once, all of
            // them, `shared` the candidate's and `unshared` not.
            let both: BTreeSet<String> = shared.iter().chain(unshared).cloned().collect();
            assert_eq!(both, own, "{name} → {candidate}: {rank:?}");
            assert_eq!(both.len(), shared.len() + unshared.len(), "{rank:?}");
            assert!(shared.iter().all(|leaf| theirs.contains(leaf)), "{rank:?}");
            assert!(
                !unshared.iter().any(|leaf| theirs.contains(leaf)),
                "{rank:?}"
            );
            // Each in the names' order.
            for list in [shared, unshared] {
                assert!(
                    list.is_sorted_by_key(|leaf| order(leaf)),
                    "{name}: {list:?}"
                );
            }
            assert_eq!(*same_feature, candidate[..8] == name[..8], "{candidate}");
            if alike {
                assert_eq!(*nearest, None, "{name} → {candidate}");
                seen.alike += 1;
            } else {
                // Rule (b): the candidate's element of the broken one's feature and kind
                // numbered nearest it, as near as any element the map's names are made from.
                let nearest = nearest
                    .as_ref()
                    .unwrap_or_else(|| panic!("{name}: {rank:?}"));
                assert!(theirs.contains(&nearest.element), "{rank:?}");
                let (owner, number) = numbered(name);
                let (of, at) = numbered(&nearest.element);
                assert_eq!(of, owner, "{rank:?}");
                assert_eq!(nearest.gap, number.abs_diff(at), "{rank:?}");
                let least = names
                    .iter()
                    .flat_map(|n| leaves(n))
                    .filter(|leaf| by_number(leaf) && numbered(leaf).0 == owner)
                    .map(|leaf| numbered(&leaf).1.abs_diff(number))
                    .min()
                    .unwrap();
                assert_eq!(nearest.gap, least, "{name}");
                seen.nearest += 1;
            }
            seen.other_feature += usize::from(!same_feature);
            seen.some_unshared += usize::from(!unshared.is_empty());
        }
        // The order follows from the ranks: more shared, then the broken name's own
        // feature's, then by name.
        let mut sorted: Vec<(&String, &Rank)> = candidates.iter().zip(ranking).collect();
        sorted.sort_by_key(|(candidate, rank)| {
            (
                std::cmp::Reverse(rank.shared.len()),
                !rank.same_feature,
                read(candidate),
            )
        });
        let sorted: Vec<&String> = sorted.into_iter().map(|(candidate, _)| candidate).collect();
        assert_eq!(sorted, candidates.iter().collect::<Vec<_>>(), "{name}");
    }
}

#[test]
fn every_broken_names_candidates_are_ranked_as_the_names_say_over_each_sweeps_map() {
    let base = box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0);
    let rectangle = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let side = rectangle_on("xz");
    let two = circles("ssssssss", "xy", &[([2.0, 2.0], 1.0), ([6.0, 2.0], 1.0)]);
    let mut seen = Seen::default();
    for (features, feature) in [
        (
            vec![rectangle.clone(), pad("pppppppp", None, "ssssssss", TEN)],
            "pppppppp",
        ),
        (
            vec![
                base.clone(),
                rectangle.clone(),
                pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN),
            ],
            "pppppppp",
        ),
        (
            vec![
                base.clone(),
                rectangle.clone(),
                pocket("pppppppp", "bbbbbbbb", "ssssssss", TEN),
            ],
            "pppppppp",
        ),
        (
            vec![
                side,
                turned(
                    "rrrrrrrr",
                    "revolve",
                    None,
                    "ssssssss",
                    "y_axis",
                    &angle("90"),
                ),
            ],
            "rrrrrrrr",
        ),
        (
            vec![base, two, pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN)],
            "qqqqqqqq",
        ),
    ] {
        let refs: Vec<&str> = features.iter().map(String::as_str).collect();
        holds(&published(&refs), feature, &mut seen);
    }
    // Both rules met, candidates of other features and with elements unshared among them:
    // the property held of something.
    assert!(
        seen.alike > 100 && seen.nearest > 10 && seen.other_feature > 10 && seen.some_unshared > 10,
        "{seen:?}"
    );
}

/// The rectangle `sweep_names` revolves, on `plane`.
fn rectangle_on(plane: &str) -> String {
    rectangle("ssssssss", plane, [1.0, 0.0], 4.0, 2.0)
}

/// A fillet `id` of `base`'s `edges`, 1 mm.
fn fillet(id: &str, base: &str, edges: &[&str]) -> String {
    let edges: Vec<String> = edges.iter().map(|e| format!("\"{e}\"")).collect();
    format!(
        r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": [{}], "radius": 1}}"#,
        edges.join(", ")
    )
}

/// The edge of the pad no element is, made from the sketch's corner pa000002 and its side
/// na000001 at once.
const BOTH: &str =
    "pppppppp.pad.Edge(G ssssssss.sketch.Vertex.pa000002, G ssssssss.sketch.Edge.na000001)";

#[test]
fn a_broken_references_ranked_candidates_are_the_same_bytes_run_to_run() {
    // A pad of a rectangle filleted at [`BOTH`], which no element is, but edges made from
    // either of its elements share one (rule a); and a box beside it filleted at its Edge13,
    // past a box's twelve (rule b). `check` in a fresh session on fake kernels of 1, 2, and
    // 4 threads, the box's step held until it's running, then released, or not held: the
    // same bytes each time. Six sessions, kernels, and caches of their own, not one run
    // read twice. Held by the kernel's holds, never by a sleep.
    use silico_core::cache::Cache;
    use silico_core::command::{Call, Sessions};
    use silico_core::kernel::{Op, Primitive};
    use silico_core::sketch::SketchSolver;
    let features = [
        rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
        pad("pppppppp", None, "ssssssss", TEN),
        fillet("f0000001", "pppppppp", &[BOTH]),
        box_at("b0000001", [50.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        fillet("f0000002", "b0000001", &["b0000001.box.Edge13"]),
    ];
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let mut envelopes = Vec::new();
    for threads in [1, 2, 4] {
        for held in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let kernel = Arc::new(FakeKernel::new(threads).unwrap());
            let hold = held
                .then(|| kernel.hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { .. }))));
            let sessions = Sessions::with_stand_ins(
                Arc::clone(&kernel),
                |cache: &std::path::Path| Cache::unsealed(cache),
                || Ok(Box::new(FakeSketches::new()) as Box<dyn SketchSolver>),
            );
            let path = dir.path().join("part.silico");
            std::fs::write(&path, document(&refs)).unwrap();
            let open = format!(
                r#"{{"path": {:?}, "create": false}}"#,
                path.to_str().unwrap()
            );
            let Call::Running(opening) = sessions.call("open", &open) else {
                panic!("no open")
            };
            assert!(opening.wait_timeout(WAIT).ok().unwrap().ok);
            let Call::Running(checking) = sessions.call(
                "check",
                r#"{"session": "s1", "model": "current", "fresh": false}"#,
            ) else {
                panic!("no check")
            };
            if let Some(hold) = hold {
                assert!(kernel.wait_running(1, WAIT));
                hold.release();
            }
            let checked = checking.wait_timeout(WAIT).ok().unwrap().text;
            envelopes.push(checked.replace(path.to_str().unwrap(), "PART"));
        }
    }
    for envelope in &envelopes[1..] {
        assert_eq!(envelope, &envelopes[0]);
    }
    // What they say, by hand from the names.
    let checked: serde_json::Value = serde_json::from_str(&envelopes[0]).unwrap();
    let problems = checked["result"]["problems"].as_array().unwrap();
    let of = |feature: &str| {
        problems
            .iter()
            .find(|p| p["feature"] == feature)
            .unwrap_or_else(|| panic!("{checked}"))
    };
    let both = of("f0000001");
    assert_eq!(both["code"], "reference_broken", "{both}");
    let (side, corner) = (
        "ssssssss.sketch.Edge.na000001",
        "ssssssss.sketch.Vertex.pa000002",
    );
    // The pad's edges made from the side (its start M, its end G) or the corner (its side
    // edge G), each sharing one of the two, the pad's own as the broken name is.
    let mut ranked: Vec<(String, serde_json::Value)> = both["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .zip(both["ranking"].as_array().unwrap())
        .map(|(c, r)| (c.as_str().unwrap().to_owned(), r.clone()))
        .collect();
    // By their text, here: the property above holds their order.
    ranked.sort_by(|a, b| a.0.cmp(&b.0));
    let rank = |shared: &str, unshared: &str| serde_json::json!({"shared": [shared], "unshared": [unshared], "same_feature": true, "nearest": null});
    assert_eq!(
        ranked,
        [
            (format!("pppppppp.pad.Edge(G {side})"), rank(side, corner)),
            (format!("pppppppp.pad.Edge(G {corner})"), rank(corner, side)),
            (format!("pppppppp.pad.Edge(M {side})"), rank(side, corner)),
        ]
    );
    let past = of("f0000002");
    assert_eq!(
        past["candidates"],
        serde_json::json!(["b0000001.box.Edge12"])
    );
    assert_eq!(
        past["ranking"],
        serde_json::json!([{"shared": [], "unshared": ["b0000001.box.Edge13"], "same_feature": true, "nearest": {"element": "b0000001.box.Edge12", "gap": 1}}])
    );
}
