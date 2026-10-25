//! break-it: #123's fix commits on the export core: the unraised comparison
//! (`needs_unraised`, `printable`'s `unraised`, the tolerance it suggests), the membrane
//! rule (a shell of unoriented faces only). Reports built by hand, as silico-kernel's
//! tests/adversarial_export.rs builds them from OCCT.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::panic::AssertUnwindSafe;

use super::export_report::{P, TETRA, report, tetra, with_shell};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use silico_core::ErrorCode;
use silico_core::export::{needs_unraised, printable};
use silico_protocol::{
    ExportMesh, MAX_EXPORT_TOLERANCE, check_export_tolerance, export_deflection, export_rounding,
};

/// `f`'s result, or its panic's message.
fn caught<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|panic| {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default()
    })
}

/// The tetrahedron with Face4 (the slanted one) `loose` mm loose, so OCCT raised it at
/// 0.1 mm, and OCCT's unraised mesh of it, the same nodes.
fn raised_tetra(loose: f64) -> (ExportMesh, ExportMesh) {
    let mut m = tetra();
    m.face_tolerances[3] = loose;
    let mut unraised = m.clone();
    unraised.force_face_deflection = true;
    (m, unraised)
}

/// The tolerance a raised face's refusal suggests, parsed from its suggestion.
fn suggested(error: &silico_core::Error) -> f64 {
    let suggestion = error.suggestion().unwrap();
    suggestion
        .strip_prefix("export with a tolerance of at least ")
        .and_then(|s| s.split_once(" mm"))
        .unwrap_or_else(|| panic!("no tolerance in {suggestion:?}"))
        .0
        .parse()
        .unwrap()
}

#[test]
fn a_raised_faces_node_at_minus_0_unraised_is_the_same_node() {
    // Break-it found (fixed): the comparison of a raised face's nodes with its unraised
    // ones went by each coordinate's bits, so a node OCCT placed at (1, 0, 0) raised and
    // (1, -0, 0) unraised was "another node", and a face whose mesh is the same once
    // written (both round to the same f32, and printable itself joins -0 with 0 for that
    // reason) was refused, its message claiming OCCT "placed other nodes than it would at
    // 0.1".
    let (m, mut unraised) = raised_tetra(0.06);
    let face4 = unraised.face_ranges()[3].clone();
    for n in face4 {
        for c in &mut unraised.nodes[n as usize] {
            if *c == 0.0 {
                *c = -0.0;
            }
        }
    }
    unraised.validate().unwrap();
    let got = printable(&m, Some(&unraised), None);
    let mesh = got.unwrap_or_else(|error| panic!("refused: {error}"));
    assert_eq!(mesh.vertices().len(), 4);
    assert_eq!(mesh.volume(), 1.0 / 6.0);
}

#[test]
fn a_raised_face_with_a_node_doubled_unraised_is_refused_naming_it() {
    // A node OCCT left twice where once raised (as it leaves two inside a cone's face at
    // times): another multiset of nodes, so another mesh of the face, and refused. Not
    // taken for the same face by its set of places.
    let (m, mut unraised) = raised_tetra(0.06);
    let face4 = unraised.face_ranges()[3].clone();
    let first = unraised.nodes[face4.start as usize];
    unraised.nodes.insert(face4.end as usize, first);
    unraised.face_nodes[3] += 1;
    unraised.validate().unwrap();
    let error = printable(&m, Some(&unraised), None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
    assert_eq!(error.elements(), ["Face4"], "{error}");
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
}

#[test]
fn a_raised_face_whose_unraised_nodes_are_a_permutation_across_faces_is_refused() {
    // Face3's and Face4's nodes swapped between them unraised: every node is still there,
    // but not on its face. Only the raised Face4 is compared, and its nodes aren't Face3's.
    let (m, mut unraised) = raised_tetra(0.06);
    let ranges = unraised.face_ranges();
    let (f3, f4) = (ranges[2].clone(), ranges[3].clone());
    for (a, b) in f3.zip(f4) {
        unraised.nodes.swap(a as usize, b as usize);
    }
    unraised.validate().unwrap();
    let error = printable(&m, Some(&unraised), None).unwrap_err();
    assert_eq!(error.elements(), ["Face4"], "{error}");
}

#[test]
fn every_face_raised_and_unchanged_is_exported_as_the_raised_mesh() {
    // All four faces loose (2 × 0.05000…1 past 0.1), the unraised mesh the same nodes but
    // other triangles' order: exported, and exactly the raised mesh's (its triangles).
    let mut m = tetra();
    for t in &mut m.face_tolerances {
        *t = 0.05f64.next_up();
    }
    assert!(needs_unraised(&m));
    let mut unraised = m.clone();
    unraised.force_face_deflection = true;
    for range in unraised.face_ranges() {
        unraised.nodes[range.start as usize..range.end as usize].rotate_left(1);
    }
    let mesh = printable(&m, Some(&unraised), None).unwrap();
    let plain = printable(&tetra(), None, None).unwrap();
    assert_eq!(mesh.triangles(), plain.triangles());
    assert_eq!(mesh.vertices(), plain.vertices());
}

#[test]
fn an_unraised_mesh_given_when_none_is_needed_changes_nothing() {
    // No face raised: the unraised mesh isn't read, even one that doesn't hold together.
    let mut broken = tetra();
    broken.force_face_deflection = true;
    broken.face_nodes[0] += 7;
    assert!(broken.validate().is_err());
    let with = printable(&tetra(), Some(&broken), None).unwrap();
    let without = printable(&tetra(), None, None).unwrap();
    assert_eq!(with.triangles(), without.triangles());
    assert_eq!(with.vertices(), without.vertices());
}

#[test]
fn a_raised_face_given_its_own_raised_mesh_as_unraised_is_a_bug_not_an_export() {
    // The raised mesh passed as its own unraised one would always compare the same.
    let (m, _) = raised_tetra(0.06);
    let error = printable(&m, Some(&m), None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal, "{error}");
}

#[test]
fn a_face_raised_by_an_infinite_or_nan_tolerance_is_refused_not_compared() {
    // needs_unraised reads the tolerances before anything is validated: +inf says yes, NaN
    // says no; either way printable refuses the report as not holding together.
    for loose in [f64::INFINITY, f64::NAN, -1.0] {
        let (m, unraised) = raised_tetra(loose);
        let _ = needs_unraised(&m);
        let error = printable(&m, Some(&unraised), None).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{loose}: {error}");
        assert!(error.cause().is_some(), "{loose}: {error}");
    }
}

/// One suggestion case: a tetrahedron reaching `reach` mm from the origin's report, at
/// `tolerance`, with Face4 `loose` mm loose and moved unraised. Returns the error.
fn refused_at(tolerance: f64, reach: f64, loose: f64) -> Option<(f64, silico_core::Error)> {
    let deflection = export_deflection(tolerance, reach).ok()?;
    if 2.0 * loose <= deflection {
        return None;
    }
    let (mut m, mut unraised) = raised_tetra(loose);
    for r in [&mut m, &mut unraised] {
        r.tolerance = tolerance;
        r.deflection = deflection;
        for d in &mut r.edge_deflections {
            *d = 0.0;
        }
        for t in &mut r.face_tolerances[..3] {
            *t = 0.0;
        }
    }
    let face4 = unraised.face_ranges()[3].start as usize;
    unraised.nodes[face4][2] = unraised.nodes[face4][2].next_up();
    if m.validate().is_err() {
        return None;
    }
    let error = printable(&m, Some(&unraised), None).unwrap_err();
    Some((deflection, error))
}

#[test]
fn the_suggested_tolerance_is_one_the_worker_wont_raise_the_face_at() {
    // The suggestion is the least tolerance, to the nanometre, at which the worker's own
    // deflection (export_deflection, with the part's reach) is at least twice the face's
    // tolerance, so OCCT doesn't raise it (BRepMesh_Deflection.cxx:149-152). Checked here
    // with the worker's rule itself, not the core's margin: the reach is what the worker
    // meshes with. Biased to where 2 × loose + rounding lands on a whole nanometre.
    // Seeded: unseeded runs found, now and then, a landing an ulp past a whole number that
    // broke it (fixed), pinned on its own below
    // (a_face_loose_to_a_nanometre_is_suggested_a_tolerance_that_unraises_it).
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 3000,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let tolerance = prop_oneof![
        Just(1e-5),
        Just(0.1),
        Just(0.01),
        Just(1.0),
        1e-5f64..1e3,
        prop::sample::select(vec![1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1.0, 10.0, 100.0]),
    ];
    let reach = prop_oneof![
        Just(1.0),
        Just(48.0),
        Just(5e4),
        1.0f64..1e6,
        prop::sample::select(vec![1.0, 10.0, 100.0, 1e3, 1e4, 1e5, 1e6]),
    ];
    // Face4's tolerance: as a fraction past half the deflection, or exactly where twice it
    // plus the rounding is a whole number of nanometres (and an ulp either side).
    let loose = prop_oneof![
        (0.5f64..20.0).prop_map(|k| (k, 0i64, 0i32)),
        (1i64..1_000_000_000_000, -1i32..2).prop_map(|(n, ulps)| (-1.0, n, ulps)),
    ];
    let on_boundary = std::cell::Cell::new(0u32);
    runner
        .run(
            &(tolerance, reach, loose),
            |(tolerance, reach, (k, n, ulps))| {
                let Ok(deflection) = export_deflection(tolerance, reach) else {
                    return Ok(());
                };
                let loose = if k > 0.0 {
                    k * deflection
                } else {
                    let mut l = (n as f64 / 1e6 - export_rounding(reach)) / 2.0;
                    for _ in 0..ulps.unsigned_abs() {
                        l = if ulps > 0 { l.next_up() } else { l.next_down() };
                    }
                    l
                };
                if !(loose.is_finite() && loose > 0.0) {
                    return Ok(());
                }
                let Some((_, error)) = refused_at(tolerance, reach, loose) else {
                    return Ok(());
                };
                prop_assert_eq!(error.code(), ErrorCode::InvalidResult, "{}", error);
                if k < 0.0 {
                    on_boundary.set(on_boundary.get() + 1);
                }
                let at = suggested(&error);
                // A tolerance the export takes, at which the part isn't refused for rounding.
                prop_assert!(
                    check_export_tolerance(at).is_ok(),
                    "{} suggested for {} at {}: not a tolerance an export takes",
                    at,
                    loose,
                    tolerance
                );
                let Ok(next) = export_deflection(at, reach) else {
                    return Err(TestCaseError::fail(format!(
                        "{at} suggested, refused for rounding at reach {reach}"
                    )));
                };
                prop_assert!(
                    2.0 * loose <= next,
                    "{} suggested for a face {} loose at {} (reach {}): the worker meshes it at \
                 {}, and OCCT raises it again",
                    at,
                    loose,
                    tolerance,
                    reach,
                    next
                );
                // Whole nanometres.
                let nanometres = (at * 1e6).round();
                prop_assert_eq!(nanometres / 1e6, at);
                Ok(())
            },
        )
        .unwrap();
    // The boundary cases were reached, not all skipped.
    assert!(on_boundary.get() > 500, "{}", on_boundary.get());
}

#[test]
fn a_face_loose_to_a_nanometre_is_suggested_a_tolerance_that_unraises_it() {
    // Break-it found (fixed): a 12.539 mm export of a part reaching 48 mm (rounding
    // r = 2.48e-6 mm) whose face is loose by 17.676940879281446 mm, so that 2 × loose + r
    // was an ulp past the 1e-9 mm grid the suggestion was rounded to. The suggestion,
    // 35.353886714 mm, had the worker mesh with 35.353881758562885 mm, an ulp short of
    // 2 × loose (35.35388175856289): OCCT raised the face again. unraising_tolerance tested
    // its figure with the margin it was given (tolerance less deflection), which differs
    // from the worker's rounding by up to an ulp of the tolerance; it now allows for that and
    // for the worker's own subtraction, on whole nanometres (1e-6 mm).
    let (tolerance, reach, loose) = (12.539018386705651, 48.0, 17.676940879281446);
    let (_, error) = refused_at(tolerance, reach, loose).unwrap();
    let at = suggested(&error);
    let next = export_deflection(at, reach).unwrap();
    assert!(
        2.0 * loose <= next,
        "{at} suggested: the worker meshes at {next}, under 2 × {loose} = {}",
        2.0 * loose
    );
}

#[test]
fn the_suggested_tolerance_holds_at_tolerances_of_kilometres_too() {
    // Break-it found (fixed): from tolerances of about 1e6 mm, the suggestion could land
    // short of twice the face's tolerance by an ulp at the worker's own deflection, so OCCT
    // raised the face again at the tolerance suggested: unraising_tolerance checked its
    // figure against the margin it was given (the tolerance less the deflection, which
    // differs from the worker's rounding by up to an ulp of the tolerance), not against the
    // subtraction the worker makes. Swept over tolerances an export takes, each face loose
    // by a fraction past half the deflection, the suggestion checked where it's given.
    let mut short = Vec::new();
    for tolerance in [1e4, 1e5, 1e6, 3e6, 9e6, 1e7, 2e7, 3e7, 4e7] {
        for reach in [1.0, 1e3, 1e6] {
            let Ok(deflection) = export_deflection(tolerance, reach) else {
                continue;
            };
            for k in [0.5000001, 0.51, 0.6, 0.75, 0.999, 1.0, 1.1, 1.2] {
                let loose = k * deflection;
                let Some((_, error)) = refused_at(tolerance, reach, loose) else {
                    continue;
                };
                let at = suggested(&error);
                if check_export_tolerance(at).is_err() {
                    continue;
                }
                match export_deflection(at, reach) {
                    Ok(next) if 2.0 * loose <= next => {}
                    other => short.push(format!(
                        "{tolerance} mm, reach {reach}, face {loose} loose: {at} suggested, \
                         worker meshes at {other:?}"
                    )),
                }
            }
        }
    }
    assert!(short.is_empty(), "{}", short.join("\n"));
}

#[test]
fn a_raised_face_at_the_largest_tolerance_isnt_told_to_export_past_it() {
    // Break-it found (fixed): at the largest tolerance an export takes (1e8 mm), a face
    // loose by more than half of it can't be fixed by any tolerance, but the suggestion
    // named one past the limit (2 × 6e7 = 1.2e8 mm), which the export refuses: an agent
    // following it was refused again, for another reason. It says to rebuild the part.
    let tolerance = MAX_EXPORT_TOLERANCE;
    let (_, error) = refused_at(tolerance, 1.0, 0.6 * tolerance).unwrap();
    let suggestion = error.suggestion().unwrap();
    let names_one_past = suggestion
        .strip_prefix("export with a tolerance of at least ")
        .and_then(|s| s.split_once(" mm"))
        .and_then(|(at, _)| at.parse::<f64>().ok())
        .is_some_and(|at| check_export_tolerance(at).is_err());
    assert!(
        !names_one_past,
        "suggests a tolerance the export refuses: {suggestion}"
    );
    assert!(
        suggestion.starts_with("rebuild the part") && suggestion.contains("no tolerance"),
        "{suggestion}"
    );
}

#[test]
fn hostile_unraised_meshes_never_panic_and_refuse_only_the_raised_faces_that_moved() {
    // Any unraised mesh that holds together and claims to be the raised one's (same
    // tolerance, deflection, faces): printable mustn't panic, and must refuse exactly the
    // raised faces whose node multisets differ, by their bits (re-derived here), or export
    // the raised mesh unchanged.
    #[derive(Debug, Clone)]
    enum Change {
        Nudge { node: usize, axis: usize, ulps: i8 },
        Swap { a: usize, b: usize },
        Reverse { face: usize },
        Duplicate { node: usize },
        Drop { node: usize },
        Zero { node: usize, axis: usize },
        Flip { node: usize },
    }
    fn change() -> impl Strategy<Value = Change> {
        prop_oneof![
            (0usize..64, 0usize..3, -2i8..3).prop_map(|(node, axis, ulps)| Change::Nudge {
                node,
                axis,
                ulps
            }),
            (0usize..64, 0usize..64).prop_map(|(a, b)| Change::Swap { a, b }),
            (0usize..4).prop_map(|face| Change::Reverse { face }),
            (0usize..64).prop_map(|node| Change::Duplicate { node }),
            (0usize..64).prop_map(|node| Change::Drop { node }),
            (0usize..64, 0usize..3).prop_map(|(node, axis)| Change::Zero { node, axis }),
            (0usize..64).prop_map(|node| Change::Flip { node }),
        ]
    }
    /// The face a node is in.
    fn face_of(u: &ExportMesh, node: usize) -> usize {
        u.face_ranges()
            .iter()
            .position(|r| r.contains(&(node as u32)))
            .unwrap()
    }
    /// `u` with one node added to or taken from `face`, its triangles and edge nodes renumbered.
    fn resize(u: &mut ExportMesh, at: usize, insert: Option<[f64; 3]>) {
        let face = face_of(u, at);
        let shift = |n: &mut u32, by: i64| *n = (i64::from(*n) + by) as u32;
        match insert {
            Some(p) => {
                u.nodes.insert(at + 1, p);
                u.face_nodes[face] += 1;
                for t in u.triangles.iter_mut().flatten() {
                    if *t as usize > at {
                        shift(t, 1);
                    }
                }
                for n in &mut u.edge_nodes {
                    if *n as usize > at {
                        shift(n, 1);
                    }
                }
            }
            None => {
                // Only a node no triangle or edge names can go.
                if u.triangles.iter().flatten().any(|&t| t as usize == at)
                    || u.edge_nodes.iter().any(|&n| n as usize == at)
                {
                    return;
                }
                u.nodes.remove(at);
                u.face_nodes[face] -= 1;
                for t in u.triangles.iter_mut().flatten() {
                    if *t as usize > at {
                        shift(t, -1);
                    }
                }
                for n in &mut u.edge_nodes {
                    if *n as usize > at {
                        shift(n, -1);
                    }
                }
            }
        }
    }
    let mut runner = TestRunner::new(Config {
        cases: 3000,
        failure_persistence: None,
        ..Config::default()
    });
    let raised = prop::collection::vec(prop::bool::ANY, 4);
    runner
        .run(
            &(raised, prop::collection::vec(change(), 0..6)),
            |(raised, changes)| {
                let mut m = tetra();
                for (f, &r) in raised.iter().enumerate() {
                    if r {
                        m.face_tolerances[f] = 0.06;
                    }
                }
                let mut u = m.clone();
                u.force_face_deflection = true;
                for c in &changes {
                    let n = u.nodes.len();
                    match *c {
                        Change::Nudge { node, axis, ulps } => {
                            let v = &mut u.nodes[node % n][axis];
                            for _ in 0..ulps.unsigned_abs() {
                                *v = if ulps > 0 { v.next_up() } else { v.next_down() };
                            }
                        }
                        Change::Swap { a, b } => u.nodes.swap(a % n, b % n),
                        Change::Reverse { face } => {
                            let r = u.face_ranges()[face].clone();
                            u.nodes[r.start as usize..r.end as usize].reverse();
                        }
                        Change::Duplicate { node } => {
                            let p = u.nodes[node % n];
                            resize(&mut u, node % n, Some(p));
                        }
                        Change::Drop { node } => resize(&mut u, node % n, None),
                        Change::Zero { node, axis } => u.nodes[node % n][axis] = 0.0,
                        Change::Flip { node } => {
                            for c in &mut u.nodes[node % n] {
                                *c = -*c;
                            }
                        }
                    }
                }
                if u.validate().is_err() {
                    return Ok(());
                }
                let got = match caught(|| printable(&m, Some(&u), None)) {
                    Err(panic) => {
                        return Err(TestCaseError::fail(format!("panicked: {panic}")));
                    }
                    Ok(got) => got,
                };
                // The oracle: each raised face's nodes, as bits (a zero of either sign as
                // 0, one place), sorted, against its own.
                let bits = |mesh: &ExportMesh, f: usize| {
                    let r = mesh.face_ranges()[f].clone();
                    let mut v: Vec<[u64; 3]> = mesh.nodes[r.start as usize..r.end as usize]
                        .iter()
                        .map(|p| p.map(|c| if c == 0.0 { 0u64 } else { c.to_bits() }))
                        .collect();
                    v.sort_unstable();
                    v
                };
                let moved: Vec<String> = (0..4)
                    .filter(|&f| raised[f] && bits(&m, f) != bits(&u, f))
                    .map(|f| format!("Face{}", f + 1))
                    .collect();
                match got {
                    Ok(mesh) => {
                        prop_assert!(moved.is_empty(), "exported with {:?} moved", moved);
                        prop_assert_eq!(mesh.vertices().len(), 4);
                        prop_assert_eq!(mesh.volume(), 1.0 / 6.0);
                    }
                    Err(error) => {
                        prop_assert!(!moved.is_empty(), "refused with none moved: {}", error);
                        prop_assert_eq!(error.code(), ErrorCode::InvalidResult, "{}", error);
                        prop_assert_eq!(error.parameter(), Some("tolerance"), "{}", error);
                        prop_assert_eq!(error.elements(), moved.as_slice(), "{}", error);
                        prop_assert!(error.suggestion().is_some(), "{}", error);
                    }
                }
                Ok(())
            },
        )
        .unwrap();
}

/// The tetrahedron (solid 0, shell 0) with `extra` unoriented faces, numbered from 5, each
/// in shell `shell`; a membrane shell is added for any `shell` past 0 (in solid `solid`).
fn with_unoriented(shell: u32, solid: u32, outer: bool, extra: u32) -> ExportMesh {
    let mut m = tetra();
    if shell > 0 {
        m.shell_solids.push(solid);
        m.shell_outer.push(outer);
        m.solids = m.solids.max(solid + 1);
    }
    for k in 0..extra {
        m.unoriented_faces.push(5 + k);
        m.unoriented_shells.push(shell);
    }
    m
}

#[test]
fn a_membrane_inside_the_solid_is_left_out_and_the_solid_exported() {
    // A shell of one INTERNAL face, as OCCT's general fuse makes of a face inside a solid,
    // flagged a cavity (its volume, nothing, isn't held to): left out, named, and the
    // tetrahedron exported whole.
    for outer in [false, true] {
        let m = with_unoriented(1, 0, outer, 2);
        m.validate().unwrap();
        let mesh = printable(&m, None, None).unwrap();
        assert_eq!(mesh.left_out(), ["Face5", "Face6"]);
        assert_eq!(mesh.volume(), 1.0 / 6.0);
        assert_eq!(mesh.triangles().len(), 4);
    }
}

#[test]
fn a_solid_of_nothing_but_a_membrane_is_refused_saying_what_was_left_out() {
    // Solid 1's only shell is a membrane: the solid encloses nothing, and the refusal says
    // its face was left out (the reason it's empty), so it isn't a bare "no volume".
    let m = with_unoriented(1, 1, true, 1);
    m.validate().unwrap();
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
    assert!(error.message().contains("encloses no volume"), "{error}");
    assert!(
        error.cause().is_some_and(|c| c.contains("Face5")),
        "{error}"
    );
    assert!(error.suggestion().is_some(), "{error}");
}

#[test]
fn a_shell_of_oriented_and_unoriented_faces_is_no_membrane() {
    // Face4 of a second tetrahedron's shell stays FORWARD, the rest are unoriented: the
    // shell has a face, so it's held to, and its one triangle is open.
    let apart: Vec<[f64; 3]> = P.iter().map(|p| p.map(|c| c + 5.0)).collect();
    let second = report(&apart, &TETRA[3..]);
    let mut m = with_shell(&tetra(), &second, 0, false);
    m.unoriented_faces = vec![6, 7, 8];
    m.unoriented_shells = vec![1, 1, 1];
    m.validate().unwrap();
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
    assert_eq!(error.elements(), ["Face5"], "{error}");
    assert!(
        error.cause().is_some_and(|c| c.contains("Face6")),
        "{error}"
    );
}

#[test]
fn an_unoriented_face_in_a_shell_that_isnt_there_is_refused() {
    let mut m = with_unoriented(0, 0, true, 1);
    m.unoriented_shells = vec![1];
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    let mut m = with_unoriented(0, 0, true, 1);
    m.unoriented_shells.clear();
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    let mut m = with_unoriented(0, 0, true, 1);
    m.unoriented_shells.push(u32::MAX);
    m.unoriented_faces.push(9);
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
}

#[test]
fn an_empty_shell_flagged_with_no_unoriented_face_is_still_refused() {
    // A shell no face is in, oriented or not, is no membrane: it encloses nothing, and is
    // refused as before (not skipped as a membrane).
    let mut m = tetra();
    m.shell_solids.push(0);
    m.shell_outer.push(false);
    m.validate().unwrap();
    let error = printable(&m, None, None).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
    assert!(error.message().contains("encloses no volume"), "{error}");
}

#[test]
fn an_unraised_mesh_at_a_tolerance_no_export_takes_is_refused_before_sending() {
    // The unraised request is held to the export's own range (1e-5 to 1e8 mm, inclusive),
    // as the raised one is: refused naming the tolerance, and nothing sent.
    use silico_core::kernel::{KernelRequest, ShapeData};
    let shape = ShapeData::checked(silico_protocol::Blob::inline(vec![]), true);
    for bad in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -0.0,
        -0.1,
        f64::MIN_POSITIVE,
        1e-5f64.next_down(),
        1e8f64.next_up(),
        f64::MAX,
    ] {
        for unraised in [false, true] {
            let request = if unraised {
                KernelRequest::unraised_export_mesh(&shape, bad)
            } else {
                KernelRequest::export_mesh(&shape, bad)
            };
            let Err(error) = request else {
                panic!("{bad} taken (unraised: {unraised})");
            };
            assert_eq!(error.code(), ErrorCode::InvalidInput, "{bad}: {error}");
            assert_eq!(error.parameter(), Some("tolerance"), "{bad}: {error}");
            assert!(error.suggestion().is_some(), "{bad}: {error}");
        }
    }
    for good in [1e-5, 0.1, 1e8] {
        assert!(KernelRequest::unraised_export_mesh(&shape, good).is_ok());
    }
}
