//! break-it: M4.8's export values (silico_protocol's export tolerance, deflection, and
//! angle) at and around their edges, and `ExportMesh::validate` on reports corrupted at
//! random: it must never panic, since it runs on whatever a worker sends.

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_protocol::{
    ExportMesh, MAX_EXPORT_TOLERANCE, MIN_EXPORT_TOLERANCE, export_angle, export_deflection,
    export_rounding,
};

#[test]
fn a_deflection_is_never_more_than_its_tolerance_whatever_the_reach() {
    // Break-it found (fixed): export_deflection takes a negative reach (the worker passes
    // an absolute value, but the function is public and says nothing of the sign) and
    // returns a deflection coarser than the tolerance: tolerance 0.1, reach -1e6 gives
    // 0.2032... A mesh within that is not within the tolerance.
    for reach in [-1e6, -1.0, -0.0, f64::MIN_POSITIVE] {
        if let Ok(deflection) = export_deflection(0.1, reach) {
            assert!(deflection <= 0.1, "reach {reach}: deflection {deflection}");
        }
    }
}

#[test]
fn a_reach_that_isnt_finite_is_refused_naming_the_tolerance() {
    for reach in [f64::NAN, f64::INFINITY] {
        let refusal = export_deflection(0.1, reach).unwrap_err();
        assert_eq!(refusal.parameter(), Some("tolerance"), "{reach}");
    }
}

#[test]
fn the_deflection_at_each_end_of_the_tolerance_range() {
    // The coarsest tolerance at the largest reach a part can have (1e6 mm, MAX_LENGTH):
    // rounding 0.103 mm off 1e8.
    let d = export_deflection(MAX_EXPORT_TOLERANCE, 1e6).unwrap();
    assert!(
        d < MAX_EXPORT_TOLERANCE && d > MAX_EXPORT_TOLERANCE - 0.2,
        "{d}"
    );
    // The finest tolerance takes rounding of up to half of it: √3·reach·2⁻²⁴ ≤ 5e-6 out to
    // 48.43 mm, so a part within 48 mm of the origin takes it, and one 49 mm out doesn't.
    assert_eq!(
        export_deflection(MIN_EXPORT_TOLERANCE, 0.0).unwrap(),
        MIN_EXPORT_TOLERANCE
    );
    assert!(export_deflection(MIN_EXPORT_TOLERANCE, 1e-3).is_ok());
    assert!(export_deflection(MIN_EXPORT_TOLERANCE, 48.0).is_ok());
    assert!(export_deflection(MIN_EXPORT_TOLERANCE, 49.0).is_err());
    // The angle is finite and in (0, 0.1] across the range.
    for t in [
        MIN_EXPORT_TOLERANCE,
        0.019,
        0.019f64.next_up(),
        0.1,
        MAX_EXPORT_TOLERANCE,
    ] {
        let a = export_angle(t);
        assert!(a > 0.0 && a <= 0.1, "{t}: {a}");
    }
    // Rounding is half a 32-bit ulp on each of three axes: at 2^k it's √3·2^(k-24).
    for k in [-10, 0, 10, 19] {
        let reach = two_to(k);
        assert_eq!(export_rounding(reach), 3f64.sqrt() * two_to(k - 24));
    }
}

/// 2^k, exactly, for k in f64's normal range.
fn two_to(k: i32) -> f64 {
    f64::from_bits(((1023 + k) as u64) << 52)
}

#[test]
fn rounding_bounds_what_32_bits_move_a_point() {
    // An independent check of export_rounding: points no further than `reach` on any axis,
    // rounded to f32, move by no more than it.
    let mut runner = TestRunner::new(Config {
        cases: 2000,
        failure_persistence: None,
        ..Config::default()
    });
    let coordinate = prop_oneof![
        -1e6..1e6f64,
        -1.0..1.0f64,
        Just(1e6),
        Just(-1e6),
        Just(0.1),
        Just(1.0 / 3.0),
        Just(f64::from(f32::MAX) / 2.0),
    ];
    runner
        .run(&prop::array::uniform3(coordinate), |p| {
            let reach = p.iter().fold(0.0f64, |m, c| m.max(c.abs()));
            let moved = p
                .iter()
                .map(|&c| {
                    let d = f64::from(c as f32) - c;
                    d * d
                })
                .sum::<f64>()
                .sqrt();
            prop_assert!(moved <= export_rounding(reach), "{p:?} moved {moved}");
            Ok(())
        })
        .unwrap();
}

/// A small report, every list filled from `values` (so the lengths and indices disagree in
/// every way the generator finds).
#[derive(Debug, Clone)]
struct Raw {
    lists: Vec<Vec<u32>>,
    floats: Vec<f64>,
    flags: Vec<bool>,
    solids: u32,
    deflection: f64,
}

fn raw() -> impl Strategy<Value = Raw> {
    let small = prop_oneof![
        0..6u32,
        Just(0),
        Just(1),
        Just(u32::MAX),
        Just(u32::MAX - 1)
    ];
    let float = prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(f64::NAN),
        Just(f64::INFINITY),
        Just(-1.0),
        Just(1e300),
        -2.0..2.0f64,
    ];
    (
        prop::collection::vec(prop::collection::vec(small, 0..8), 13),
        prop::collection::vec(float.clone(), 0..12),
        prop::collection::vec(any::<bool>(), 0..6),
        0..4u32,
        float,
    )
        .prop_map(|(lists, floats, flags, solids, deflection)| Raw {
            lists,
            floats,
            flags,
            solids,
            deflection,
        })
}

fn report(r: &Raw) -> ExportMesh {
    let l = |i: usize| r.lists[i].clone();
    let floats = |n: usize| r.floats.iter().copied().cycle().take(n).collect::<Vec<_>>();
    let nodes = r.lists[0]
        .iter()
        .map(|&n| u64::from(n))
        .sum::<u64>()
        .min(16) as usize;
    ExportMesh {
        tolerance: 0.1,
        deflection: r.deflection,
        nodes: floats(nodes * 3)
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .chain(std::iter::repeat([0.0; 3]))
            .take(nodes)
            .collect(),
        face_nodes: l(0),
        triangles: l(1).chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect(),
        face_triangles: l(2),
        face_numbers: l(3),
        face_deflections: floats(r.lists[3].len()),
        face_shells: l(4),
        face_tolerances: floats(r.lists[3].len()),
        unoriented_faces: l(6).into_iter().map(|f| f.saturating_add(1000)).collect(),
        unoriented_shells: l(6).into_iter().map(|_| 0).collect(),
        shell_solids: l(5),
        shell_outer: r.flags.clone(),
        solids: r.solids,
        loose_faces: l(6),
        edge_faces: l(7),
        edge_numbers: l(8),
        edge_degenerate: r.flags.clone(),
        edge_vertices: l(9).chunks_exact(2).map(|v| [v[0], v[1]]).collect(),
        edge_deflections: floats(r.lists[7].len()),
        edge_node_counts: l(10),
        edge_nodes: l(11),
    }
}

#[test]
fn validate_never_panics_and_what_it_takes_can_be_indexed() {
    let mut runner = TestRunner::new(Config {
        cases: 5000,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(&raw(), |r| {
            let m = report(&r);
            let checked = std::panic::catch_unwind(|| m.validate());
            prop_assert!(checked.is_ok(), "validate panicked on {m:?}");
            if checked.unwrap().is_ok() {
                // Every index the core uses, in range.
                let faces = m.face_nodes.len();
                let ranges = m.face_ranges();
                prop_assert_eq!(ranges.len(), faces);
                for t in &m.triangles {
                    prop_assert!(t.iter().all(|&n| (n as usize) < m.nodes.len()));
                }
                for &s in &m.face_shells {
                    prop_assert!((s as usize) < m.shell_solids.len());
                }
                for &s in &m.shell_solids {
                    prop_assert!(s < m.solids);
                }
                for (&f, nodes) in m.edge_faces.iter().zip(m.edge_uses()) {
                    prop_assert!((f as usize) < faces);
                    for n in nodes {
                        prop_assert!(ranges[f as usize].contains(n));
                    }
                }
                prop_assert_eq!(m.edge_uses().count(), m.edge_faces.len());
            }
            Ok(())
        })
        .unwrap();
}
