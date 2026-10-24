//! break-it: M4.8's export core on OCCT's meshes of hostile parts: every primitive at
//! scales from a micron to metres and tolerances across the range, measured against its
//! analytic surface (not OCCT's own figures); degenerate edges (poles, apexes, a horn
//! torus's pinch); solids that touch themselves; parts tiny beside their distance from the
//! origin; tolerances at the range's ends; a solid inside out; the same export in fresh and
//! used workers; and the archive read back by Python's own zip and XML readers (FreeCAD's).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::exported;
use super::kernel;

use exported::{data, exported};
use kernel::kernel;
use silico_core::ErrorCode;
use silico_core::export::{Object, PrintableMesh, printable, three_mf};
use silico_kernel::{BooleanOp, Primitive, boolean, make, make_box, translate};
use silico_protocol::{ChamferSize, export_rounding};

fn wide(v: [f32; 3]) -> [f64; 3] {
    v.map(f64::from)
}

/// The largest distance from `surface` (an unsigned distance to the part's boundary) of
/// any of 28 points spread over each triangle (its corners, and a grid of six steps along
/// each side): a lower bound on the mesh's deviation, so one past the tolerance is a mesh
/// past it.
fn deviation(mesh: &PrintableMesh, surface: &dyn Fn([f64; 3]) -> f64) -> f64 {
    let k = 6;
    let mut worst = 0.0f64;
    for t in mesh.triangles() {
        let [a, b, c] = t.map(|v| wide(mesh.vertices()[v as usize]));
        for i in 0..=k {
            for j in 0..=k - i {
                let (u, v) = (f64::from(i) / f64::from(k), f64::from(j) / f64::from(k));
                let w = 1.0 - u - v;
                let p = [0, 1, 2].map(|n| w * a[n] + u * b[n] + v * c[n]);
                worst = worst.max(surface(p));
            }
        }
    }
    worst
}

/// The distance from `p` to the segment `a`-`b`, in a plane.
fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0] * d[0] + d[1] * d[1];
    let s = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len).clamp(0.0, 1.0);
    libm::hypot(p[0] - a[0] - s * d[0], p[1] - a[1] - s * d[1])
}

/// The unsigned distance from `p` to the boundary of a solid of revolution about Z whose
/// half profile (radius, height) is the polyline `profile` (its segments on the axis left
/// out, as they bound nothing).
fn to_revolved(profile: &[[f64; 2]], p: [f64; 3]) -> f64 {
    let q = [libm::hypot(p[0], p[1]), p[2]];
    profile
        .windows(2)
        .filter(|s| !(s[0][0] == 0.0 && s[1][0] == 0.0))
        .map(|s| to_segment(q, s[0], s[1]))
        .fold(f64::INFINITY, f64::min)
}

/// A primitive's boundary as an unsigned distance function, analytic.
fn surface_of(primitive: &Primitive) -> Box<dyn Fn([f64; 3]) -> f64> {
    match *primitive {
        Primitive::Sphere { radius } => Box::new(move |p: [f64; 3]| {
            ((p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - radius).abs()
        }),
        Primitive::Torus {
            major_radius,
            minor_radius,
        } => Box::new(move |p: [f64; 3]| {
            (libm::hypot(libm::hypot(p[0], p[1]) - major_radius, p[2]) - minor_radius).abs()
        }),
        Primitive::Cylinder { radius, height } => {
            let profile = [[0.0, 0.0], [radius, 0.0], [radius, height], [0.0, height]];
            Box::new(move |p| to_revolved(&profile, p))
        }
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => {
            let profile = [[0.0, 0.0], [radius1, 0.0], [radius2, height], [0.0, height]];
            Box::new(move |p| to_revolved(&profile, p))
        }
        Primitive::Box { .. } => panic!("not a curved primitive"),
    }
}

/// How many triangles meshing `primitive`'s largest round at `tolerance` takes, roughly:
/// (2π / step)², the step the finer of the angular rule and the chord's sagitta.
fn estimated_triangles(primitive: &Primitive, tolerance: f64) -> f64 {
    let r = match *primitive {
        Primitive::Sphere { radius } => radius,
        Primitive::Torus { major_radius, .. } => major_radius,
        Primitive::Cylinder { radius, .. } => radius,
        Primitive::Cone {
            radius1, radius2, ..
        } => radius1.max(radius2),
        Primitive::Box { .. } => 1.0,
    };
    let step = silico_protocol::export_angle(tolerance).min((8.0 * tolerance / r).sqrt());
    (std::f64::consts::TAU / step) * (std::f64::consts::TAU / step)
}

/// The largest sampled distance from `surface` of the triangles of a report as the worker
/// sent it (64-bit nodes, before welding).
fn raw_deviation(raw: &silico_protocol::ExportMesh, surface: &dyn Fn([f64; 3]) -> f64) -> f64 {
    let k = 6;
    let mut worst = 0.0f64;
    for t in &raw.triangles {
        let [a, b, c] = t.map(|n| raw.nodes[n as usize]);
        for i in 0..=k {
            for j in 0..=k - i {
                let (u, v) = (f64::from(i) / f64::from(k), f64::from(j) / f64::from(k));
                let w = 1.0 - u - v;
                worst = worst.max(surface([0, 1, 2].map(|n| w * a[n] + u * b[n] + v * c[n])));
            }
        }
    }
    worst
}

#[test]
fn every_round_primitive_is_exported_exactly_when_its_mesh_is_within_the_tolerance() {
    // Rule 7, measured against each primitive's own surface rather than OCCT's figures, at
    // scales from 10 µm to metres: a mesh taken must be within the tolerance as a reader gets
    // it (32-bit vertices), and a mesh refused as "further than the tolerance" must be. A
    // refusal of a mesh within 0.8 of the tolerance (the samples are a lower bound on the
    // deviation, so a margin) is a part refused for nothing.
    // Break-it found flat cones refused at every ordinary tolerance though their meshes were
    // hundreds of times inside it: OCCT's face estimate (Poly_Triangulation::Deflection, a
    // sample at each triangle's centroid in the face's parameters, not the nearest point)
    // is far off on a cone (for r 300 mm, h 1 mm, 1.277 mm at a 1 mm tolerance, where the
    // mesh is 1.0e-3 mm from it). Fixed as FreeCAD has it: a face's estimate isn't compared
    // with the tolerance. Where OCCT's mesh is in fact past the tolerance, the export takes
    // it as FreeCAD's does, OCCT's result: pinned here as `KNOWN`, so another miss fails.
    let primitives = [
        Primitive::Cylinder {
            radius: 5.0,
            height: 10.0,
        },
        Primitive::Cylinder {
            radius: 1000.0,
            height: 1.0,
        },
        Primitive::Cylinder {
            radius: 1e-2,
            height: 100.0,
        },
        Primitive::Cone {
            radius1: 5.0,
            radius2: 0.0,
            height: 10.0,
        },
        Primitive::Cone {
            radius1: 0.0,
            radius2: 300.0,
            height: 1.0,
        },
        Primitive::Cone {
            radius1: 0.0,
            radius2: 30.0,
            height: 1.0,
        },
        Primitive::Cone {
            radius1: 30.0,
            radius2: 28.0,
            height: 1.0,
        },
        Primitive::Cone {
            radius1: 2.0,
            radius2: 1.0,
            height: 50.0,
        },
        Primitive::Torus {
            major_radius: 10.0,
            minor_radius: 2.0,
        },
        Primitive::Torus {
            major_radius: 300.0,
            minor_radius: 1.0,
        },
    ];
    let mut wrong = Vec::new();
    let mut worker = kernel();
    for primitive in &primitives {
        let shape = data(&make(primitive).unwrap());
        let surface = surface_of(primitive);
        for tolerance in [1e-2, 0.1, 1.0, 10.0] {
            if estimated_triangles(primitive, tolerance) > 2e6 {
                continue;
            }
            let raw = worker.export_mesh(&shape, tolerance).unwrap();
            match printable(&raw, None) {
                Ok(mesh) => {
                    let d = deviation(&mesh, &*surface);
                    if d > tolerance {
                        wrong.push(format!(
                            "{primitive:?} at {tolerance}: taken, {d} from the surface"
                        ));
                    }
                }
                Err(error) if error.message().contains("further than the tolerance") => {
                    let d = raw_deviation(&raw, &*surface);
                    if d <= 0.8 * tolerance {
                        wrong.push(format!(
                            "{primitive:?} at {tolerance}: refused, but {d} from the surface \
                             (OCCT's face figures {:?})",
                            raw.face_deflections
                        ));
                    }
                }
                Err(error) => wrong.push(format!("{primitive:?} at {tolerance}: {error}")),
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_spheres_mesh_misses_the_tolerance_at_its_seam_by_what_occt_makes() {
    // Decided (m4.8-export-fable, item 2): the export tolerance is the mesher's deflection, as
    // FreeCAD's, and a face's interior is OCCT's to meet. On a sphere it doesn't: OCCT's
    // sphere mesher does no deflection control (BRepMesh_MeshAlgoFactory.cxx:74-77), and by
    // the seam its Delaunay loses the grid's nearest nodes, so a band of triangles there
    // spans 1.7 grid steps, their sagitta up to 2.1 times the tolerance. Taken, as FreeCAD's
    // export takes it, and pinned here, worst deviation over tolerance, measured as the
    // round-primitives test measures: the same ratios FreeCAD 1.1.3's mesher gives with
    // these parameters (m4.8-export-fable's table), so a change in OCCT or in the export's
    // parameters shows.
    let cases: [(f64, f64, f64); 7] = [
        (5.0, 0.01, 0.386),
        (20.0, 0.01, 1.545),
        (50.0, 0.01, 1.471),
        (100.0, 0.01, 2.078),
        (50.0, 0.1, 1.314),
        (300.0, 0.1, 0.761),
        (1000.0, 0.1, 2.078),
    ];
    let mut worker = kernel();
    let mut got = Vec::new();
    for (radius, tolerance, _) in cases {
        let sphere = Primitive::Sphere { radius };
        let raw = worker
            .export_mesh(&data(&make(&sphere).unwrap()), tolerance)
            .unwrap();
        let mesh = printable(&raw, None).unwrap();
        let ratio = deviation(&mesh, &*surface_of(&sphere)) / tolerance;
        got.push((radius, tolerance, (ratio * 1e3).round() / 1e3));
    }
    assert_eq!(got, cases);
}

#[test]
fn a_cylinder_with_its_rim_chamfered_exports_at_ordinary_tolerances() {
    // A 60 mm disc 10 mm thick, one rim chamfered 2 mm: the chamfer is a cone's frustum, the
    // commonest cone there is. Refused only if its mesh is in fact past the tolerance of the
    // part, measured against the part's own profile (refused within 0.8 of it is a break).
    // Break-it found (fixed): at 0.01 mm, refused naming the chamfer's face (Face2): OCCT's
    // figure for it is 0.0114 mm, but its mesh is 0.0068 mm from the cone at most.
    let disc = make(&Primitive::Cylinder {
        radius: 30.0,
        height: 10.0,
    })
    .unwrap();
    let profiles = [
        [
            [0.0, 0.0],
            [30.0, 0.0],
            [30.0, 8.0],
            [28.0, 10.0],
            [0.0, 10.0],
        ],
        [
            [0.0, 0.0],
            [28.0, 0.0],
            [30.0, 2.0],
            [30.0, 10.0],
            [0.0, 10.0],
        ],
    ];
    let mut worker = kernel();
    let mut wrong = Vec::new();
    let mut chamfered = 0;
    for edge in 1..=3 {
        let Ok(built) =
            silico_kernel::chamfer(&disc, &[edge], ChamferSize::Equal { distance: 2.0 }, false)
        else {
            continue;
        };
        chamfered += 1;
        let shape = data(&built.shape);
        for tolerance in [0.1, 0.01] {
            let raw = worker.export_mesh(&shape, tolerance).unwrap();
            // The profile the part's nodes lie on.
            let profile = profiles
                .iter()
                .find(|profile| {
                    raw.nodes
                        .iter()
                        .all(|&p| to_revolved(&profile[..], p) <= 1e-9)
                })
                .expect("the nodes lie on one of the two profiles");
            let surface = |p: [f64; 3]| to_revolved(&profile[..], p);
            match printable(&raw, None) {
                Ok(mesh) => {
                    let d = deviation(&mesh, &surface);
                    assert!(d <= tolerance, "Edge{edge} at {tolerance}: {d}");
                }
                Err(error) => {
                    let d = raw_deviation(&raw, &surface);
                    if d <= 0.8 * tolerance {
                        wrong.push(format!(
                            "Edge{edge} at {tolerance}: {d} from the part, refused: {error}"
                        ));
                    }
                }
            }
        }
    }
    assert!(chamfered > 0, "no rim could be chamfered");
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_horn_torus_pinched_at_the_origin_exports_or_is_refused_naming_its_face() {
    // The tube touches the axis: the surface pinches to a point at the origin that is no
    // vertex of the B-rep, so no edge welds the nodes OCCT puts there.
    let horn = make(&Primitive::Torus {
        major_radius: 5.0,
        minor_radius: 5.0,
    })
    .unwrap();
    for tolerance in [0.1, 1e-2] {
        match exported(&horn, tolerance) {
            Ok(mesh) => {
                let d = deviation(
                    &mesh,
                    &*surface_of(&Primitive::Torus {
                        major_radius: 5.0,
                        minor_radius: 5.0,
                    }),
                );
                assert!(d <= tolerance, "{d}");
                // A horn torus encloses 2π²Rr² = 2467.4 mm³, the mesh a little less.
                let exact = 2.0 * std::f64::consts::PI * std::f64::consts::PI * 125.0;
                assert!(mesh.volume() < exact && mesh.volume() > 0.98 * exact);
            }
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
                assert_eq!(error.elements(), ["Face1"], "{error}");
            }
        }
    }
}

#[test]
fn a_pointed_cones_apex_is_one_vertex_whichever_end_it_is_at() {
    for (r1, r2) in [(5.0, 0.0), (0.0, 5.0)] {
        let cone = make(&Primitive::Cone {
            radius1: r1,
            radius2: r2,
            height: 10.0,
        })
        .unwrap();
        let mesh = exported(&cone, 0.1).unwrap();
        let apex = [0.0, 0.0, if r1 == 0.0 { 0.0 } else { 10.0 }];
        let at = mesh
            .vertices()
            .iter()
            .filter(|v| {
                let v = wide(**v);
                (v[0] - apex[0]).abs() < 1e-6
                    && (v[1] - apex[1]).abs() < 1e-6
                    && (v[2] - apex[2]).abs() < 1e-6
            })
            .count();
        assert_eq!(at, 1, "apex {apex:?}");
        // Inscribed: under πr²h/3.
        let exact = std::f64::consts::PI * 25.0 * 10.0 / 3.0;
        assert!(
            mesh.volume() < exact && mesh.volume() > 0.99 * exact,
            "{}",
            mesh.volume()
        );
    }
}

#[test]
fn tiny_and_coarse_rounds_still_close() {
    // A 1 µm sphere at the coarsest tolerance, a 1 µm torus, and a sphere at 1e8 mm: the
    // angular rule alone sets how fine they are.
    for (primitive, tolerance) in [
        (Primitive::Sphere { radius: 1e-3 }, 0.1),
        (Primitive::Sphere { radius: 1e-3 }, 1e8),
        (Primitive::Sphere { radius: 5.0 }, 1e8),
        (
            Primitive::Torus {
                major_radius: 2e-3,
                minor_radius: 1e-3,
            },
            0.1,
        ),
        (
            Primitive::Cylinder {
                radius: 1e-3,
                height: 1e-3,
            },
            1e8,
        ),
    ] {
        let shape = make(&primitive).unwrap();
        let mesh = exported(&shape, tolerance).unwrap_or_else(|e| panic!("{primitive:?}: {e}"));
        assert!(mesh.volume() > 0.0, "{primitive:?}");
        let d = deviation(&mesh, &*surface_of(&primitive));
        assert!(d <= tolerance, "{primitive:?}: {d}");
    }
}

#[test]
fn a_sphere_cavity_is_bounded_by_the_analytic_volumes() {
    // A 10 mm cube less a sphere of 3 mm at its center: one solid, its cavity a sphere
    // with two poles and a seam, reversed. The cavity's mesh is inscribed in the sphere, so
    // the solid's mesh holds at least 1000 − 4/3·π·3³, and at most that of a sphere the
    // tolerance smaller.
    let cube = make_box(10.0, 10.0, 10.0).unwrap();
    let ball = translate(&make(&Primitive::Sphere { radius: 3.0 }).unwrap(), [5.0; 3])
        .unwrap()
        .shape;
    let hollow = boolean(BooleanOp::Cut, &cube, &ball).unwrap().shape;
    let ball_volume = |r: f64| 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
    for tolerance in [0.1, 1e-2] {
        let mesh = exported(&hollow, tolerance).unwrap();
        let v = mesh.volume();
        assert!(v > 1000.0 - ball_volume(3.0), "{tolerance}: {v}");
        assert!(
            v < 1000.0 - ball_volume(3.0 - tolerance),
            "{tolerance}: {v}"
        );
    }
}

#[test]
fn a_solid_touching_itself_along_an_edge_is_refused_as_not_manifold() {
    // A 2 × 2 × 1 block less its (0, 0) and (1, 1) quarters: two cubes meeting along the
    // line x = y = 1. As one solid, that edge is on four faces, and the mesh's on four
    // triangles; as two solids, two closed meshes. Either way, never a mesh with an edge
    // on four triangles.
    let block = make_box(2.0, 2.0, 1.0).unwrap();
    let quarter = make_box(1.0, 1.0, 1.0).unwrap();
    let other = translate(&quarter, [1.0, 1.0, 0.0]).unwrap().shape;
    let cut = boolean(BooleanOp::Cut, &block, &quarter).unwrap().shape;
    let cut = boolean(BooleanOp::Cut, &cut, &other).unwrap().shape;
    let raw = kernel().export_mesh(&data(&cut), 0.1).unwrap();
    match printable(&raw, None) {
        Ok(mesh) => {
            assert_eq!(raw.solids, 2, "one solid, taken");
            assert_eq!(mesh.vertices().len(), 16);
            assert_eq!(mesh.volume(), 2.0);
        }
        Err(error) => {
            assert_eq!(raw.solids, 1);
            assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
            assert!(error.message().contains("isn't manifold"), "{error}");
            assert_eq!(error.elements_total(), 4, "{error}");
        }
    }
}

#[test]
fn a_part_tiny_beside_its_distance_from_the_origin_is_refused_for_what_its_rounding_does() {
    // A slab 1e-5 mm thick 1000 mm out: in 32 bits (spacing 6.1e-5 there) its two faces
    // round onto one plane, so the mesh a reader gets encloses nothing. The kernel meshed it
    // closed and outward; what fails is the rounding, which a finer tolerance only makes
    // worse.
    // Break-it found (fixed): refused as "the faces named face inward: their shell encloses
    // the wrong side", suggesting "export with a finer tolerance": the volume in 32 bits is
    // 0, which rule 5 reads as inward (it runs before rule 6), and neither the message nor
    // the advice says the part is too thin to hold its shape in 32 bits this far out.
    let slab = translate(&make_box(1e-5, 1.0, 1.0).unwrap(), [1000.0, 0.0, 0.0])
        .unwrap()
        .shape;
    let error = exported(&slab, 0.1).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult, "{error}");
    assert!(!error.message().contains("inward"), "{error}");
    assert!(error.message().contains("32 bits"), "{error}");
    let suggestion = error.suggestion().unwrap_or_default();
    assert!(!suggestion.contains("finer tolerance"), "{error}");
}

#[test]
fn the_finest_tolerance_is_taken_until_the_rounding_is_half_of_it() {
    // 1e-5 mm is the finest tolerance taken, and the rounding to 32 bits may take at most
    // half of it: √3·reach·2⁻²⁴ ≤ 5e-6 out to a reach of 48.4 mm. A unit cube at the origin
    // (reach 1) exports at it; moved 49 mm out (reach 50) it's refused, the tolerance named.
    // (Break-it: 1e-5 was refused for any part off the origin.)
    let cube = make_box(1.0, 1.0, 1.0).unwrap();
    let mesh = exported(&cube, 1e-5).unwrap();
    assert!((mesh.volume() - 1.0).abs() <= 1e-12, "{}", mesh.volume());
    let out = translate(&cube, [49.0, 0.0, 0.0]).unwrap().shape;
    let error = exported(&out, 1e-5).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
    assert!(exported(&out, 1.1e-5).is_ok());
}

#[test]
fn a_part_at_the_largest_reach_exports_at_a_tolerance_its_rounding_allows() {
    // 1e6 mm out (MAX_LENGTH): rounding moves a point by up to 0.103 mm. Refused at 0.1,
    // taken at 1.
    let far = translate(&make_box(1.0, 1.0, 1.0).unwrap(), [999_999.0, 0.0, 0.0])
        .unwrap()
        .shape;
    let error = exported(&far, 0.1).unwrap_err();
    assert_eq!(error.parameter(), Some("tolerance"), "{error}");
    let mesh = exported(&far, 1.0).unwrap();
    assert!((mesh.volume() - 1.0).abs() < 1e-6, "{}", mesh.volume());
    assert!(export_rounding(1e6) > 0.1);
}

#[test]
fn every_edge_of_a_box_filleted_or_chamfered_exports_closed() {
    // Corner blends where three fillets meet, and chamfers nearly through (but not the
    // slivers OCCT can't mesh), at tolerances fine and coarse.
    let block = make_box(10.0, 20.0, 30.0).unwrap();
    let all: Vec<u32> = (1..=12).collect();
    let rounded = silico_kernel::fillet(&block, &all, 4.999).unwrap().shape;
    let bevelled =
        silico_kernel::chamfer(&block, &all, ChamferSize::Equal { distance: 4.9 }, false)
            .unwrap()
            .shape;
    let box_volume = 6000.0;
    for (name, shape) in [("rounded", &rounded), ("bevelled", &bevelled)] {
        for tolerance in [1e-2, 0.1, 1.0, 100.0] {
            let mesh =
                exported(shape, tolerance).unwrap_or_else(|e| panic!("{name} {tolerance}: {e}"));
            assert!(
                mesh.volume() > 0.0 && mesh.volume() < box_volume,
                "{name} {tolerance}: {}",
                mesh.volume()
            );
        }
    }
}

#[test]
fn the_same_shape_exports_the_same_report_in_any_worker_after_any_work() {
    // Fresh workers, and one that meshed other shapes (and this one for display) first.
    let block = make_box(10.0, 20.0, 30.0).unwrap();
    let rounded = silico_kernel::fillet(&block, &[1, 2, 3, 4], 3.0)
        .unwrap()
        .shape;
    let shape = data(&rounded);
    let first = kernel().export_mesh(&shape, 0.05).unwrap();
    let mut used = kernel();
    used.export_mesh(
        &data(&make(&Primitive::Sphere { radius: 7.0 }).unwrap()),
        0.01,
    )
    .unwrap();
    used.tessellate(&shape, 0.5).unwrap();
    used.export_mesh(&shape, 0.5).unwrap();
    assert_eq!(used.export_mesh(&shape, 0.05).unwrap(), first);
    assert_eq!(kernel().export_mesh(&shape, 0.05).unwrap(), first);
}

#[test]
fn an_export_refused_leaves_the_worker_able_to_export() {
    // After each refusal (a tolerance the worker refuses, a part too far out for it), the
    // same worker exports a good part.
    let mut worker = kernel();
    let cube = data(&make_box(1.0, 1.0, 1.0).unwrap());
    let far = data(
        &translate(&make_box(1.0, 1.0, 1.0).unwrap(), [5e4, 0.0, 0.0])
            .unwrap()
            .shape,
    );
    assert!(worker.export_mesh(&far, 1e-3).is_err());
    assert!(worker.export_mesh(&far, 1e-2).is_err());
    let mesh = worker.export_mesh(&cube, 0.1).unwrap();
    let volume = printable(&mesh, None).unwrap().volume();
    assert!((volume - 1.0).abs() <= 1e-12, "{volume}");
}

#[test]
fn a_solid_inside_out_is_not_exported_inside_out() {
    // FreeCAD's Part.Shape.reverse() on a box: the same faces, every one reversed, as a
    // STEP file or a broken boolean can give. Its one shell bounds the outside: either
    // refused, or exported facing outward with the box's volume, never a mesh of -1.
    let dir = tempfile::Builder::new()
        .prefix("silico-export-reversed-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("reversed.brep");
    super::oracle::freecad(&format!(
        r#"
import json, Part
b = Part.makeBox(1, 1, 1)
b.reverse()
b.exportBrep({path:?})
print("SILICO_ORACLE " + json.dumps({{"volume": b.Volume, "orientation": b.Orientation}}))
"#,
        path = path.to_str().unwrap()
    ));
    let text = std::fs::read(&path).unwrap();
    let Ok(shape) = silico_kernel::from_text_brep(&text) else {
        // Refused on reading: never exported.
        return;
    };
    match exported(&shape, 0.1) {
        Ok(mesh) => assert!(
            (mesh.volume() - 1.0).abs() < 1e-12,
            "exported with volume {}",
            mesh.volume()
        ),
        Err(error) => assert!(
            matches!(
                error.code(),
                ErrorCode::InvalidResult | ErrorCode::InvalidInput
            ),
            "{error}"
        ),
    }
}

#[test]
fn an_archive_reads_back_through_pythons_zip_and_xml_with_every_name_exact() {
    // FreeCAD's Python: zipfile checks every entry's CRC and sizes; ElementTree parses the
    // model; each object's name and part number must read back as written, characters a
    // parser would otherwise normalize (tab, line feed, carriage return) included.
    let mesh = exported(&make_box(1.0, 2.0, 3.0).unwrap(), 0.1).unwrap();
    let names = [
        "plain",
        "",
        "  spaced  ",
        "tab\there",
        "line\nfeed",
        "cr\r\nlf",
        "]]> & <tag> \"quoted\" 'single'",
        "&#9; literal",
        "del\u{7f} nel\u{85} ls\u{2028}",
        "ü 漢字 \u{FFFD} \u{10FFFF} 🦀",
    ];
    let objects: Vec<Object<'_>> = names
        .iter()
        .map(|&name| Object {
            name,
            part_number: name,
            mesh: &mesh,
        })
        .collect();
    let archive = three_mf(&objects).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("silico-export-names-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("names.3mf");
    std::fs::write(&path, &archive).unwrap();
    let read = super::oracle::freecad(&format!(
        r#"
import json, zipfile
import xml.etree.ElementTree as ET
z = zipfile.ZipFile({path:?})
bad = z.testzip()
infos = [(i.filename, i.compress_type, i.date_time) for i in z.infolist()]
root = ET.fromstring(z.read("3D/3dmodel.model"))
ns = {{"m": "http://schemas.microsoft.com/3dmanufacturing/core/2015/02"}}
objects = root.findall("m:resources/m:object", ns)
print("SILICO_ORACLE " + json.dumps({{
    "bad": bad,
    "infos": infos,
    "names": [o.get("name") for o in objects],
    "parts": [o.get("partnumber") for o in objects],
    "vertices": [len(o.findall("m:mesh/m:vertices/m:vertex", ns)) for o in objects],
    "items": len(root.findall("m:build/m:item", ns)),
}}))
"#,
        path = path.to_str().unwrap()
    ));
    assert_eq!(read["bad"], serde_json::Value::Null, "{read}");
    let names_read: Vec<String> = serde_json::from_value(read["names"].clone()).unwrap();
    let parts_read: Vec<String> = serde_json::from_value(read["parts"].clone()).unwrap();
    assert_eq!(names_read, names, "{read}");
    assert_eq!(parts_read, names, "{read}");
    assert_eq!(read["items"], names.len(), "{read}");
    for v in read["vertices"].as_array().unwrap() {
        assert_eq!(v.as_u64(), Some(mesh.vertices().len() as u64));
    }
    for info in read["infos"].as_array().unwrap() {
        assert_eq!(info[1], 0, "stored: {read}");
        assert_eq!(info[2], serde_json::json!([1980, 1, 1, 0, 0, 0]), "{read}");
    }
}

#[test]
fn a_tolerance_too_fine_to_mesh_in_time_ends_as_a_worker_timeout_naming_the_export() {
    // A 1 mm sphere at 1e-3 mm, a hundred times the finest tolerance taken: FreeCAD's
    // angular rule (0.01 rad there, whatever the part's size) makes 805,418 triangles,
    // which OCCT takes about three minutes to make (FreeCAD 1.1.3's own export of it, on
    // the same OCCT, makes 805,416 facets in 201 s). Decided (m4.8-export-fable, item 1): no
    // exact rule says before meshing how many triangles OCCT will make, and a deadline would
    // depend on the machine, so the request runs to its time limit and fails loudly, as a
    // worker_timeout naming the export. The deterministic limit is a count of the triangles
    // made, in M4.1's table of the tools' limits (docs/ports/mesh-export.md, Open).
    use std::time::Duration;
    let sphere = data(&make(&Primitive::Sphere { radius: 1.0 }).unwrap());
    let request = silico_core::kernel::KernelRequest::export_mesh(&sphere, 1e-3)
        .unwrap()
        .with_timeout(Duration::from_secs(2));
    let error = kernel().run(request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::WorkerTimeout, "{error}");
    assert!(error.message().contains("ExportMesh"), "{error}");
}

#[test]
fn a_countersunk_hole_exports_at_a_fine_tolerance_its_mesh_meets() {
    // A 60 × 40 × 20 block with a hole of 4 mm radius countersunk to 10 mm over its top 6
    // mm: the cone is the countersink's face. At 0.01 mm, refused only if the cone's own
    // triangles are past the tolerance of the cone, measured against it: in (ρ, z) about the
    // hole's axis, the line through (4, 14) and (10, 20), so a point's distance is
    // |(ρ − 4) − (z − 14)| / √2.
    // Break-it found (fixed): refused naming the cone's face (OCCT's figure for it past
    // 0.01 mm), though its triangles are within a fraction of the tolerance of the cone.
    let block = make_box(60.0, 40.0, 20.0).unwrap();
    let cone = translate(
        &make(&Primitive::Cone {
            radius1: 4.0,
            radius2: 10.0,
            height: 6.0,
        })
        .unwrap(),
        [30.0, 20.0, 14.0],
    )
    .unwrap()
    .shape;
    let hole = translate(
        &make(&Primitive::Cylinder {
            radius: 4.0,
            height: 20.0,
        })
        .unwrap(),
        [30.0, 20.0, 0.0],
    )
    .unwrap()
    .shape;
    let sunk = boolean(BooleanOp::Cut, &block, &cone).unwrap().shape;
    let sunk = boolean(BooleanOp::Cut, &sunk, &hole).unwrap().shape;
    let to_cone = |p: [f64; 3]| {
        let rho = libm::hypot(p[0] - 30.0, p[1] - 20.0);
        ((rho - 4.0) - (p[2] - 14.0)).abs() / std::f64::consts::SQRT_2
    };
    for tolerance in [0.1, 0.05, 0.01] {
        let raw = kernel().export_mesh(&data(&sunk), tolerance).unwrap();
        let Err(error) = printable(&raw, None) else {
            continue;
        };
        // The cone's face: the one whose nodes all lie on the cone.
        let ranges = raw.face_ranges();
        let mut start = 0usize;
        let mut cone_deviation = None;
        for (f, &count) in raw.face_triangles.iter().enumerate() {
            let end = start + count as usize;
            let range = ranges[f].clone();
            let on_cone = range
                .clone()
                .all(|n| to_cone(raw.nodes[n as usize]) <= 1e-9);
            if on_cone && !range.is_empty() {
                let mut worst = 0.0f64;
                for t in &raw.triangles[start..end] {
                    let [a, b, c] = t.map(|n| raw.nodes[n as usize]);
                    for i in 0..=6 {
                        for j in 0..=6 - i {
                            let (u, v) = (f64::from(i) / 6.0, f64::from(j) / 6.0);
                            let w = 1.0 - u - v;
                            worst = worst
                                .max(to_cone([0, 1, 2].map(|n| w * a[n] + u * b[n] + v * c[n])));
                        }
                    }
                }
                cone_deviation = Some((raw.face_numbers[f], worst));
            }
            start = end;
        }
        let (face, d) = cone_deviation.expect("a face on the cone");
        assert!(
            d > 0.8 * tolerance,
            "at {tolerance}: refused, but Face{face}, the cone, is {d} from it: {error}"
        );
    }
}

#[test]
fn a_nist_part_whose_cones_mesh_with_coincident_nodes_exports_as_freecad_exports_it() {
    // NIST's CTC 01 (AP242), as FreeCAD's exportBrep wrote it (tests/fixtures/brep). At
    // FreeCAD's default 0.1 mm, OCCT meshes two of its cones (Face58, Face60, countersinks)
    // with two nodes at exactly the same point inside the face, so four triangles have no
    // area even in 64 bits (a side of length 0). FreeCAD 1.1.3's export of it merges the
    // coincident points and drops those triangles (BRepMesh.cpp:275-279): 10,876 facets on
    // 5,420 points, closed, manifold, and none of no area.
    // Break-it found (fixed): refused, "a triangle of the faces named has no area once its
    // vertices are rounded to the 32 bits a 3MF reader takes (elements Face58, Face60)",
    // suggesting a finer tolerance (which fails too): the weld joins nodes only through the
    // B-rep's edges and vertices, so OCCT's coincident nodes inside a face are never
    // joined. Of NIST's parts, CTC 01 and FTC 06 (AP242) fail this way at the default
    // tolerance.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brep");
    let text = std::fs::read(dir.join("nist_ctc_01_asme1_ap242-e1.brep")).unwrap();
    let shape = silico_kernel::from_text_brep(&text).unwrap();
    let mesh = exported(&shape, 0.1).unwrap();
    // FreeCAD's 10,876 facets on 5,420 points.
    assert_eq!(mesh.triangles().len(), 10_876);
    assert_eq!(mesh.vertices().len(), 5_420);
}

/// A B-rep OCCT wrote (tests/fixtures/brep), with Silico's checksum.
fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/brep")
        .join(name);
    let mut body = std::fs::read(path).unwrap();
    body.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
    body
}

#[test]
fn a_box_whose_face_is_unoriented_is_refused_as_open_saying_the_face_was_left_out() {
    // Decided (m4.8-export-fable, item 5): INTERNAL and EXTERNAL faces bound no change of
    // material, so they're left out, as OCCT's own volume leaves them out. A box whose x = 0
    // face is INTERNAL or EXTERNAL (tests/fixtures/brep) is then open as material: refused
    // as open, naming the four faces around the hole (those with a node on x = 0), the cause
    // saying which face was left out.
    for name in ["box_internal_face1.bin", "box_external_face1.bin"] {
        let shape = silico_kernel::from_brep(&fixture(name)).unwrap();
        let raw = kernel().export_mesh(&data(&shape), 0.1).unwrap();
        assert_eq!(raw.unoriented_faces.len(), 1, "{name}");
        let ranges = raw.face_ranges();
        let mut around: Vec<String> = (0..raw.face_numbers.len())
            .filter(|&f| ranges[f].clone().any(|n| raw.nodes[n as usize][0] == 0.0))
            .map(|f| format!("Face{}", raw.face_numbers[f]))
            .collect();
        around.sort();
        assert_eq!(around.len(), 4, "{name}: {around:?}");
        let error = printable(&raw, None).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidResult, "{name}: {error}");
        assert!(
            error.message().contains("the mesh is open"),
            "{name}: {error}"
        );
        let mut named = error.elements().to_vec();
        named.sort();
        assert_eq!(named, around, "{name}: {error}");
        let left = format!("Face{}", raw.unoriented_faces[0]);
        assert!(
            error
                .cause()
                .is_some_and(|c| c.contains("left out") && c.contains(&left)),
            "{name}: {error}"
        );
    }
}

#[test]
fn a_membrane_inside_a_box_is_left_out_and_the_box_exported_whole() {
    // A 10 × 20 × 30 box fused with a square inside it at x = 5 (FreeCAD 1.1.3's
    // generalFuse, which keeps the square as an INTERNAL face of the box, valid): the
    // membrane is left out, and the box is the box, 12 triangles on 8 vertices enclosing
    // 6000. (FreeCAD's Part.Solid drops an unoriented face, so it can't make one.)
    let dir = tempfile::Builder::new()
        .prefix("silico-export-membrane-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("membrane.brep");
    super::oracle::freecad(&format!(
        r#"
import json, FreeCAD, Part
V = FreeCAD.Vector
m = Part.Face(Part.makePolygon([V(5, 5, 5), V(5, 15, 5), V(5, 15, 25), V(5, 5, 25), V(5, 5, 5)]))
fused, _ = Part.makeBox(10, 20, 30).generalFuse([m])
s = fused.Solids[0]
assert s.isValid() and [f.Orientation for f in s.Faces].count("Internal") == 1
s.exportBrep({path:?})
print("SILICO_ORACLE " + json.dumps({{"faces": len(s.Faces)}}))
"#,
        path = path.to_str().unwrap()
    ));
    let shape = silico_kernel::from_text_brep(&std::fs::read(&path).unwrap()).unwrap();
    let raw = kernel().export_mesh(&data(&shape), 0.1).unwrap();
    assert_eq!(raw.unoriented_faces, [7]);
    let mesh = printable(&raw, None).unwrap();
    assert_eq!(mesh.triangles().len(), 12);
    assert_eq!(mesh.vertices().len(), 8);
    assert_eq!(mesh.volume(), 6000.0);
    assert_eq!(mesh.left_out(), [7]);
}

#[test]
fn a_face_meshed_coarser_than_asked_for_its_own_tolerance_is_refused_naming_the_tolerance() {
    // Decided (m4.8-export-fable, item 2b): OCCT meshes a face no finer than twice its
    // tolerance (BRepMesh_Deflection.cxx:146-147). A 50 mm sphere whose sub-shapes FreeCAD
    // gave a tolerance of 0.02 (fixTolerance): at 0.01 OCCT meshes it at 0.04, 3.9 times the
    // tolerance (the port review's run), so it's refused, naming Face1 and the tolerance; at
    // 0.04, 2·0.02 = 0.04 is past the deflection, 0.04 less the rounding, and it's refused;
    // at 0.05 it's taken.
    let dir = tempfile::Builder::new()
        .prefix("silico-export-loose-tolerance-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("sphere.brep");
    super::oracle::freecad(&format!(
        r#"
import json, Part
s = Part.makeSphere(50)
s.fixTolerance(0.02)
s.exportBrep({path:?})
print("SILICO_ORACLE " + json.dumps({{"tolerance": s.getTolerance(1)}}))
"#,
        path = path.to_str().unwrap()
    ));
    let shape = silico_kernel::from_text_brep(&std::fs::read(&path).unwrap()).unwrap();
    for tolerance in [0.01, 0.04] {
        let raw = kernel().export_mesh(&data(&shape), tolerance).unwrap();
        let error = printable(&raw, None).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::InvalidResult,
            "{tolerance}: {error}"
        );
        assert_eq!(error.parameter(), Some("tolerance"), "{tolerance}: {error}");
        assert_eq!(error.elements(), ["Face1"], "{tolerance}: {error}");
        assert!(
            error.message().contains("at 0.04 mm") && error.message().contains("to 0.02 mm"),
            "{tolerance}: {error}"
        );
    }
    let raw = kernel().export_mesh(&data(&shape), 0.05).unwrap();
    printable(&raw, None).unwrap();
}
