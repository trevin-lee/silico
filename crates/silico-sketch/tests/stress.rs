//! M3.1: PlaneGCS's determinism (ARCHITECTURE.md, invariant 5). Generated sketches,
//! solved, must give results identical bit for bit to the committed reference,
//! tests/determinism/sketch-stress-reference.json, which was written on one platform and
//! is checked on every other: the Mac and the Linux CI runners run this same test.
//!
//! The sketches are rectangles, closed polygons with lengths and angles, slots of lines
//! and arcs, rectangles given one constraint too many (redundant or conflicting), and a
//! grid of rectangles big enough that Levenberg–Marquardt's `JᵀJ` sums more than Eigen's
//! unblocked depth (about 336), where Eigen's blocking would follow each CPU's caches if
//! the worker didn't fix them. Each is drawn off its solution, from a fixed seed, and
//! solved with each algorithm.
//!
//! To write the reference again (after a deliberate change to PlaneGCS, its patches, or
//! the recipe, for review): SILICO_WRITE_SKETCH_STRESS=1 cargo nextest run -p silico-sketch
//! --test stress.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/systems.rs"]
mod systems;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use sha2::{Digest, Sha256};
use silico_protocol::{
    Algorithm, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem, SolveStatus, Solved,
    SolverConstraint,
};
use systems::{Builder, Rng};

const REFERENCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/determinism/sketch-stress-reference.json"
);
const SEED: u64 = 0x5111_c05c_e7c4;
const CASES: usize = 240;
const ALGORITHMS: [Algorithm; 3] = [
    Algorithm::DogLeg,
    Algorithm::LevenbergMarquardt,
    Algorithm::Bfgs,
];

/// A rectangle at (x, y), `w` by `h`, its corners drawn up to `off` from their places, its
/// sides joined, horizontal and vertical, and its lengths given: returns its sides.
fn rectangle(b: &mut Builder, rng: &mut Rng, x: f64, y: f64, w: f64, h: f64) -> [Geometry; 4] {
    let off = 0.05 * w.min(h);
    let mut jitter = |v: f64| v + rng.range(-off, off);
    let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    let mut sides = [Geometry(0); 4];
    for i in 0..4 {
        let (a, c) = (corners[i], corners[(i + 1) % 4]);
        sides[i] = b.line((jitter(a.0), jitter(a.1)), (jitter(c.0), jitter(c.1)));
    }
    for i in 0..4 {
        let (a, next) = (sides[i], sides[(i + 1) % 4]);
        b.constrain(|tag| SolverConstraint::Coincident {
            tag,
            a: PointOf::End(a),
            b: PointOf::Start(next),
        });
    }
    let [bottom, right, top, left] = sides;
    b.constrain(|tag| SolverConstraint::Horizontal { tag, line: bottom });
    b.constrain(|tag| SolverConstraint::Horizontal { tag, line: top });
    b.constrain(|tag| SolverConstraint::Vertical { tag, line: right });
    b.constrain(|tag| SolverConstraint::Vertical { tag, line: left });
    let (wv, hv) = (b.fixed(w), b.fixed(h));
    b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Start(bottom),
        b: PointOf::End(bottom),
        value: wv,
    });
    b.constrain(|tag| SolverConstraint::DistanceY {
        tag,
        a: PointOf::Start(right),
        b: PointOf::End(right),
        value: hv,
    });
    sides
}

fn pin(b: &mut Builder, point: PointOf, x: f64, y: f64) {
    let fixed = b.fixed_point(x, y);
    b.constrain(|tag| SolverConstraint::Coincident {
        tag,
        a: point,
        b: PointOf::Point(fixed),
    });
}

fn a_rectangle(rng: &mut Rng) -> Builder {
    let mut b = Builder::default();
    let (x, y) = (rng.range(-500.0, 500.0), rng.range(-500.0, 500.0));
    let (w, h) = (rng.range(0.01, 900.0), rng.range(0.01, 900.0));
    let sides = rectangle(&mut b, rng, x, y, w, h);
    pin(&mut b, PointOf::Start(sides[0]), x, y);
    b
}

/// A closed polygon of 3 to 8 sides, each side's length given and some corners' angles,
/// one corner pinned: under-, fully, or over-constrained by chance.
fn a_polygon(rng: &mut Rng) -> Builder {
    let mut b = Builder::default();
    let n = 3 + rng.below(6) as usize;
    let r = rng.range(1.0, 300.0);
    let corners: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            // Points on a circle, by rotating a unit vector exactly by rationals: no trig.
            let t = (i as f64 + rng.range(0.0, 0.4)) / n as f64;
            let (c, s) = (1.0 - 2.0 * t * t, 2.0 * t * (1.0 - t * t).max(0.0));
            (r * c + rng.range(-1.0, 1.0), r * s + rng.range(-1.0, 1.0))
        })
        .collect();
    let sides: Vec<Geometry> = (0..n)
        .map(|i| b.line(corners[i], corners[(i + 1) % n]))
        .collect();
    for i in 0..n {
        let (a, next) = (sides[i], sides[(i + 1) % n]);
        b.constrain(|tag| SolverConstraint::Coincident {
            tag,
            a: PointOf::End(a),
            b: PointOf::Start(next),
        });
        if rng.below(3) > 0 {
            let length = b.fixed(rng.range(1.0, 200.0));
            b.constrain(|tag| SolverConstraint::Distance {
                tag,
                a: PointOf::Start(a),
                b: PointOf::End(a),
                value: length,
            });
        }
        if rng.below(2) == 0 {
            let angle = b.fixed(rng.range(0.3, 2.8));
            b.constrain(|tag| SolverConstraint::Angle {
                tag,
                a,
                b: next,
                value: angle,
            });
        }
    }
    pin(&mut b, PointOf::Start(sides[0]), corners[0].0, corners[0].1);
    b
}

/// A slot: two horizontal lines joined by two arcs, a radius, a length, one end pinned.
fn a_slot(rng: &mut Rng) -> Builder {
    let mut b = Builder::default();
    let r = rng.range(1.0, 50.0);
    let len = rng.range(1.0, 200.0);
    let j = |rng: &mut Rng| rng.range(-0.05, 0.05) * r;
    let bottom = b.line((j(rng), -r + j(rng)), (len + j(rng), -r + j(rng)));
    let top = b.line((len + j(rng), r + j(rng)), (j(rng), r + j(rng)));
    let arc = |b: &mut Builder, rng: &mut Rng, cx: f64, a0: f64, a1: f64| {
        let center = PointValues {
            x: b.unknown(cx + j(rng)),
            y: b.unknown(j(rng)),
        };
        let radius = b.unknown(r * (1.0 + rng.range(-0.05, 0.05)));
        let start_angle = b.unknown(a0 + rng.range(-0.05, 0.05));
        let end_angle = b.unknown(a1 + rng.range(-0.05, 0.05));
        let start = b.point_values(cx + j(rng), -r + j(rng));
        let end = b.point_values(cx + j(rng), r + j(rng));
        let g = b.add(SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start,
            end,
        });
        b.constraints.push(SolverConstraint::ArcRules { arc: g });
        g
    };
    let half = std::f64::consts::FRAC_PI_2;
    let right = arc(&mut b, rng, len, -half, half);
    let left = arc(&mut b, rng, 0.0, half, 3.0 * half);
    for (a, c) in [
        (PointOf::End(bottom), PointOf::Start(right)),
        (PointOf::End(right), PointOf::Start(top)),
        (PointOf::End(top), PointOf::Start(left)),
        (PointOf::End(left), PointOf::Start(bottom)),
    ] {
        b.constrain(|tag| SolverConstraint::Coincident { tag, a, b: c });
    }
    b.constrain(|tag| SolverConstraint::Horizontal { tag, line: bottom });
    b.constrain(|tag| SolverConstraint::Parallel {
        tag,
        a: bottom,
        b: top,
    });
    let (rv, lv) = (b.fixed(r), b.fixed(len));
    b.constrain(|tag| SolverConstraint::Radius {
        tag,
        circle: right,
        value: rv,
    });
    b.constrain(|tag| SolverConstraint::Distance {
        tag,
        a: PointOf::Start(bottom),
        b: PointOf::End(bottom),
        value: lv,
    });
    pin(&mut b, PointOf::Start(bottom), 0.0, -r);
    b
}

/// A rectangle with one constraint more: a direction again (redundant), a width that
/// disagrees (conflicting), or a width that agrees to within the redundancy tolerance.
fn a_rectangle_and_one_more(rng: &mut Rng) -> Builder {
    let mut b = Builder::default();
    let w = rng.range(1.0, 100.0);
    let h = rng.range(1.0, 100.0);
    let sides = rectangle(&mut b, rng, 0.0, 0.0, w, h);
    pin(&mut b, PointOf::Start(sides[0]), 0.0, 0.0);
    match rng.below(4) {
        0 => {
            b.constrain(|tag| SolverConstraint::Horizontal {
                tag,
                line: sides[2],
            });
        }
        1 => {
            b.constrain(|tag| SolverConstraint::Parallel {
                tag,
                a: sides[1],
                b: sides[3],
            });
        }
        k => {
            let other = b.fixed(if k == 2 { w + 1.0 } else { w + 1e-6 });
            b.constrain(|tag| SolverConstraint::DistanceX {
                tag,
                a: PointOf::Start(sides[2]),
                b: PointOf::End(sides[2]),
                value: other,
            });
        }
    }
    b
}

/// A grid of rectangles, each sharing its bottom-left corner with its neighbour's
/// corner: one connected system of `n` × `n` rectangles (12 equations each, more than
/// Eigen's unblocked depth from 5 × 5 on).
fn a_grid(rng: &mut Rng, n: usize) -> Builder {
    let mut b = Builder::default();
    let mut grid: Vec<Vec<[Geometry; 4]>> = Vec::new();
    for i in 0..n {
        let mut row = Vec::new();
        for j in 0..n {
            let (x, y) = (12.0 * j as f64, 12.0 * i as f64);
            let (w, h) = (rng.range(4.0, 10.0), rng.range(4.0, 10.0));
            row.push(rectangle(&mut b, rng, x, y, w, h));
        }
        grid.push(row);
    }
    let gap = b.fixed(12.0);
    let zero = b.fixed(0.0);
    for i in 0..n {
        for j in 0..n {
            let here = grid[i][j][0];
            if j > 0 {
                let left = grid[i][j - 1][0];
                b.constrain(|tag| SolverConstraint::DistanceX {
                    tag,
                    a: PointOf::Start(left),
                    b: PointOf::Start(here),
                    value: gap,
                });
                b.constrain(|tag| SolverConstraint::DistanceY {
                    tag,
                    a: PointOf::Start(left),
                    b: PointOf::Start(here),
                    value: zero,
                });
            } else if i > 0 {
                let below = grid[i - 1][j][0];
                b.constrain(|tag| SolverConstraint::DistanceY {
                    tag,
                    a: PointOf::Start(below),
                    b: PointOf::Start(here),
                    value: gap,
                });
                b.constrain(|tag| SolverConstraint::DistanceX {
                    tag,
                    a: PointOf::Start(below),
                    b: PointOf::Start(here),
                    value: zero,
                });
            }
        }
    }
    pin(&mut b, PointOf::Start(grid[0][0][0]), 0.0, 0.0);
    b
}

fn digest(solved: &Solved) -> String {
    let mut h = Sha256::new();
    for v in &solved.values {
        h.update(v.to_bits().to_le_bytes());
    }
    // The residuals too: computed from the solution, so as platform-dependent as it.
    for (tag, r) in &solved.residuals {
        h.update(tag.0.to_le_bytes());
        h.update(r.map_or(u64::MAX, f64::to_bits).to_le_bytes());
    }
    let mut hex = String::with_capacity(16);
    for byte in &h.finalize()[..8] {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn tags(tags: &[silico_protocol::Tag]) -> String {
    let list: Vec<String> = tags.iter().map(|t| t.0.to_string()).collect();
    format!("[{}]", list.join(","))
}

/// One line of the reference: everything the solve says, its values as a digest of
/// their bits.
fn line(name: &str, system: &SketchSystem, solved: &Solved) -> String {
    let status = match solved.status {
        SolveStatus::Success => "Success",
        SolveStatus::Converged => "Converged",
        SolveStatus::SolvedExceptRedundant => "SolvedExceptRedundant",
        SolveStatus::Failed => "Failed",
        SolveStatus::SolvedInvalid => "SolvedInvalid",
    };
    let free: Vec<String> = solved.free.iter().map(|v| v.0.to_string()).collect();
    format!(
        "  \"{name}\": {{\"values\": {}, \"constraints\": {}, \"status\": \"{status}\", \"dofs\": {}, \"conflicting\": {}, \"redundant\": {}, \"partially_redundant\": {}, \"free\": [{}], \"solution\": \"{}\"}}",
        system.values.len(),
        system.constraints.len(),
        solved.dofs,
        tags(&solved.conflicting),
        tags(&solved.redundant),
        tags(&solved.partially_redundant),
        free.join(","),
        digest(solved)
    )
}

fn corpus() -> Vec<(String, SketchSystem)> {
    let mut rng = Rng(SEED);
    let mut cases = Vec::new();
    for i in 0..CASES {
        let kind = i % 4;
        let b = match kind {
            0 => a_rectangle(&mut rng),
            1 => a_polygon(&mut rng),
            2 => a_slot(&mut rng),
            _ => a_rectangle_and_one_more(&mut rng),
        };
        let algorithm = ALGORITHMS[(i / 4) % 3];
        cases.push((format!("case{i:03}_kind{kind}"), b.system(algorithm)));
    }
    for (n, algorithm) in [
        (4, Algorithm::LevenbergMarquardt),
        (6, Algorithm::LevenbergMarquardt),
        (6, Algorithm::DogLeg),
        (8, Algorithm::LevenbergMarquardt),
    ] {
        let b = a_grid(&mut rng, n);
        cases.push((format!("grid{n}x{n}_{algorithm:?}"), b.system(algorithm)));
    }
    cases
}

/// Every generated sketch solves to the reference's bits; and the corpus covers what it
/// claims: every status but `Failed` (that too, by chance), conflicts and redundancies, and
/// a Levenberg–Marquardt system deeper than Eigen's unblocked product.
#[test]
fn generated_sketches_solve_to_the_references_bits() {
    let mut lines = BTreeMap::new();
    let mut statuses = [false; 5];
    let (mut conflicts, mut redundancies, mut deepest) = (0, 0, 0usize);
    for (name, system) in corpus() {
        system
            .check()
            .unwrap_or_else(|e| panic!("{name}: {}", e.message));
        let solved = silico_sketch::solve(&system).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        statuses[solved.status as usize] = true;
        conflicts += usize::from(!solved.conflicting.is_empty());
        redundancies += usize::from(!solved.redundant.is_empty());
        if system.algorithms == [Algorithm::LevenbergMarquardt] {
            deepest = deepest.max(system.constraints.len());
        }
        lines.insert(name.clone(), line(&name, &system, &solved));
    }
    assert!(
        statuses[0] && statuses[1] && statuses[2],
        "statuses seen: {statuses:?}"
    );
    assert!(
        conflicts >= 5 && redundancies >= 5,
        "{conflicts} {redundancies}"
    );
    assert!(
        deepest > 336,
        "the deepest LM system has {deepest} constraints"
    );

    let text = format!(
        "{{\n{}\n}}\n",
        lines.values().cloned().collect::<Vec<_>>().join(",\n")
    );
    if std::env::var_os("SILICO_WRITE_SKETCH_STRESS").is_some() {
        std::fs::write(REFERENCE, &text).unwrap();
        return;
    }
    let reference = std::fs::read_to_string(REFERENCE).unwrap();
    if reference != text {
        let want: Vec<&str> = reference.lines().collect();
        let got: Vec<&str> = text.lines().collect();
        let differing =
            want.iter().zip(&got).filter(|(w, g)| w != g).count() + want.len().abs_diff(got.len());
        let first: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(w, g)| w != g)
            .take(5)
            .map(|(w, g)| format!("reference {w}\n     this {g}"))
            .collect();
        panic!(
            "{differing} of {} lines differ from {REFERENCE} (first 5):\n{}",
            want.len(),
            first.join("\n")
        );
    }
}

#[test]
fn a_failed_algorithm_falls_back_to_the_next() {
    // From the tests review: a slot BFGS fails on alone and DogLeg solves; tried in that
    // order, the second solves it, to DogLeg's own bits.
    let (_, system) = corpus()
        .into_iter()
        .find(|(n, _)| n == "case010_kind2")
        .unwrap();
    assert_eq!(system.algorithms, [Algorithm::Bfgs]);
    let alone = silico_sketch::solve(&system).unwrap();
    assert_eq!(alone.status, SolveStatus::Failed);
    assert_eq!(alone.algorithm, None);
    let mut dl = system.clone();
    dl.algorithms = vec![Algorithm::DogLeg];
    let dl = silico_sketch::solve(&dl).unwrap();
    assert_eq!(dl.status, SolveStatus::Success);
    let mut both = system.clone();
    both.algorithms = vec![Algorithm::Bfgs, Algorithm::DogLeg];
    let both = silico_sketch::solve(&both).unwrap();
    assert_eq!(both.status, SolveStatus::Success);
    assert_eq!(both.algorithm, Some(Algorithm::DogLeg));
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&both.values), bits(&dl.values));
}
