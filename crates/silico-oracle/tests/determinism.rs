//! determinism_sweep (docs/roadmap/m1-kernel.md, M1.9): every oracle case, and the suite's
//! generated ones, built twice on fresh kernel workers (the second in reverse order, after
//! the others) gives byte-identical B-reps, histories (each result's fates, counts, and
//! ends, and, beside them, its topology), summaries, FreeCAD-style measures, and meshes;
//! and so does every sketch-feature case (M3.6), its document rebuilt twice by recompute.
//! Their digests match a committed reference, recorded on the Mac, so CI on Linux checks
//! that both platforms build every case the same, bit for bit. Rerecord (only with the
//! maintainer's agreement, as for tests/determinism/stress-reference.json) with
//! `SILICO_DETERMINISM_RECORD=1 cargo nextest run -p silico-oracle --test determinism`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use silico_core::command::rebuild;
use silico_core::kernel::{ElementCounts, Ends, History, InputHistory, KernelWorker, ShapeData};
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Recompute};
use silico_oracle::{Case, features, generate, load, silico::build_with_history};

const CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/oracle");
const REFERENCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/determinism/oracle-reference.json"
);

/// FNV-1a, 64 bits: a digest that's the same everywhere.
#[derive(Default)]
struct Digest(u64);

impl Digest {
    fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        if self.0 == 0 {
            self.0 = 0xcbf2_9ce4_8422_2325;
        }
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    fn f64s(&mut self, values: impl IntoIterator<Item = f64>) -> &mut Self {
        for v in values {
            self.bytes(&v.to_bits().to_le_bytes());
        }
        self
    }

    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// A history's fates, counts, and ends, serialized as the History before M2.7 was (in
/// that order), so their digest is the one recorded then.
#[derive(Serialize)]
struct Fates<'a> {
    inputs: &'a [InputHistory],
    result: &'a ElementCounts,
    ends: &'a Ends,
}

/// `value`'s JSON, digested.
fn digest_json(value: &impl Serialize) -> String {
    Digest::default()
        .bytes(&serde_json::to_vec(value).unwrap())
        .hex()
}

/// What building `case` on `kernel` gives, digested: its B-rep, the history its last
/// operation reported (the order of OCCT's lists, which element names are built from, in
/// M2: its fates, counts, and ends as `history`, and its result's topology as `topology`),
/// its summary, FreeCAD-style measures, and mesh (deflection 0.1), each or the code of the
/// error that refused it.
fn digests(kernel: &mut KernelWorker, case: &Case) -> BTreeMap<String, String> {
    match build_with_history(kernel, &case.shape) {
        Ok((shape, history)) => digests_of(kernel, &shape, history.as_ref()),
        Err(e) => BTreeMap::from([("refused".to_owned(), e.code().as_str().to_owned())]),
    }
}

/// What a shape built with `history` gives, digested, as [`digests`] says.
fn digests_of(
    kernel: &mut KernelWorker,
    shape: &ShapeData,
    history: Option<&History>,
) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    if let Some(history) = history {
        // Exhaustive: a new field must be given a key.
        let History {
            inputs,
            result,
            ends,
            topology,
        } = history;
        found.insert(
            "history".to_owned(),
            digest_json(&Fates {
                inputs,
                result,
                ends,
            }),
        );
        found.insert("topology".to_owned(), digest_json(topology));
    }
    found.insert(
        "brep".to_owned(),
        Digest::default().bytes(shape.brep().as_bytes()).hex(),
    );
    let refused = |e: silico_core::Error| format!("refused: {}", e.code().as_str());
    let s = match kernel.summarize(shape) {
        Ok(s) => s,
        Err(e) => {
            found.insert("summary".to_owned(), refused(e));
            return found;
        }
    };
    let mut d = Digest::default();
    d.f64s([s.volume, s.area])
        .f64s(s.center)
        .bytes(
            &[s.faces, s.edges, s.vertices]
                .map(u32::to_le_bytes)
                .concat(),
        )
        .bytes(&[u8::from(s.valid)]);
    found.insert("summary".to_owned(), d.hex());
    let measures = kernel.measure_as_freecad(shape).map(|m| {
        let mut d = Digest::default();
        d.f64s([m.volume, m.area])
            .f64s(m.center)
            .f64s(m.bounds.into_iter().flatten().flatten());
        d.hex()
    });
    found.insert("measures".to_owned(), measures.unwrap_or_else(refused));
    let mesh = match kernel.tessellate(shape, 0.1) {
        Ok(mesh) => mesh,
        Err(e) => {
            found.insert("mesh".to_owned(), refused(e));
            return found;
        }
    };
    let mut d = Digest::default();
    d.f64s(mesh.positions.iter().flatten().copied())
        .f64s(mesh.normals.iter().flatten().map(|&n| f64::from(n)))
        .bytes(
            &mesh
                .triangles
                .iter()
                .flatten()
                .chain(&mesh.face_triangles)
                .chain(&mesh.edge_point_counts)
                .flat_map(|i| i.to_le_bytes())
                .collect::<Vec<u8>>(),
        )
        .f64s(mesh.edge_points.iter().flatten().copied());
    found.insert("mesh".to_owned(), d.hex());
    found
}

type Digests = BTreeMap<String, BTreeMap<String, String>>;

/// Every case's digests, built in order on a fresh worker.
fn run(cases: &[Case]) -> Digests {
    let mut kernel = KernelWorker::start().unwrap();
    cases
        .iter()
        .map(|c| (c.name.clone(), digests(&mut kernel, c)))
        .collect()
}

/// Every sketch-feature case's digests (M3.6): the document rebuilt by recompute on a fresh
/// pool and sketch worker, each case's solid as its feature built it (its last step's
/// history), digested on a fresh worker, under the case's name.
fn run_features() -> Digests {
    let cases = features::load(Path::new(CASES)).unwrap().unwrap();
    let engine = Recompute::new(KernelPool::start().unwrap());
    let model = rebuild(&engine, cases.document.clone()).unwrap().unwrap();
    let mut kernel = KernelWorker::start().unwrap();
    cases
        .cases
        .iter()
        .map(|case| {
            let digests = match model.outcome(&case.feature) {
                Some(FeatureOutcome::Built { built, .. }) => {
                    digests_of(&mut kernel, &built.shape, built.history.as_ref())
                }
                other => BTreeMap::from([("refused".to_owned(), format!("{other:?}"))]),
            };
            (case.name.clone(), digests)
        })
        .collect()
}

#[test]
fn every_oracle_case_builds_the_same_every_time_and_everywhere() {
    let mut cases = load(Path::new(CASES)).unwrap();
    cases.extend(generate::cases(20_261_010, 40));
    let mut first = run(&cases);
    let mut reversed = cases.clone();
    reversed.reverse();
    let second = run(&reversed);
    for (name, digests) in &first {
        assert_eq!(
            Some(digests),
            second.get(name),
            "{name} built differently on a fresh worker"
        );
    }
    // The sketch-feature cases, the document rebuilt twice, each by a fresh engine.
    let features = run_features();
    assert_eq!(
        features,
        run_features(),
        "a sketch-feature case built differently"
    );
    for (name, digests) in features {
        assert!(
            first.insert(name.clone(), digests).is_none(),
            "{name} is a case twice"
        );
    }
    if std::env::var_os("SILICO_DETERMINISM_RECORD").is_some() {
        let json = serde_json::to_string_pretty(&first).unwrap();
        std::fs::write(REFERENCE, json + "\n").unwrap();
        return;
    }
    let reference: Digests =
        serde_json::from_str(&std::fs::read_to_string(REFERENCE).unwrap()).unwrap();
    let differ: Vec<String> = first
        .iter()
        .filter(|(name, digests)| reference.get(*name) != Some(digests))
        .map(|(name, digests)| format!("{name}: {digests:?}, reference {:?}", reference.get(name)))
        .chain(
            reference
                .keys()
                .filter(|name| !first.contains_key(*name))
                .map(|name| format!("{name}: in the reference, not built")),
        )
        .collect();
    assert!(
        differ.is_empty(),
        "{} cases differ from the reference:\n{}",
        differ.len(),
        differ.join("\n")
    );
}
