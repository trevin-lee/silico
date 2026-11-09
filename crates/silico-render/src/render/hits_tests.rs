//! [`hits`], Select Other's cast (M5.7), worked by hand: what lies under a pointer, nearest
//! the eye first, each element numbered as FreeCAD numbers a Part box (and as the kernel's
//! tests pin it: `silico-kernel/tests/mesh.rs`, `topology.rs`, `adversarial_distance.rs`).

use std::sync::Arc;

use silico_protocol::{Camera, MAX_RENDER_COORDINATE, Mesh, Projection};

use super::{
    Element, HIT_EDGE_POINTS, HIT_VERTEX_POINTS, Hit, MAX_HITS, MIN_EYE_DISTANCE, Pickable,
    VERTEX_REACH, hits, project, ray,
};
use crate::bench::box_mesh;

/// A box from the origin, `l` by `w` by `h`, numbered as FreeCAD numbers a Part box: its
/// faces x low, x high, y low, y high, z low, z high ([`box_mesh`]'s, Face1 to Face6); its
/// vertices [`box_mesh`]'s (Vertex1 (0, 0, h), Vertex2 the origin, ..., Vertex7 (l, w, h),
/// Vertex8 (l, w, 0)); and its edges in OCCT's order, not [`box_mesh`]'s: Face1's four
/// (Edge1 along z at y = 0, Edge2 along y at z = h, Edge3 along z at y = w, Edge4 along y at
/// z = 0), Face2's (Edge5 to Edge8, the same at x = l), then Edge9 and Edge10 along x at
/// y = 0 (z = 0, then h), Edge11 and Edge12 along x at y = w (z = 0, then h).
fn freecad_box(l: f64, w: f64, h: f64) -> Mesh {
    let mut mesh = box_mesh([0.0; 3], [l, w, h]);
    let edges: [([f64; 3], [f64; 3]); 12] = [
        ([0.0, 0.0, h], [0.0, 0.0, 0.0]),
        ([0.0, 0.0, h], [0.0, w, h]),
        ([0.0, w, 0.0], [0.0, w, h]),
        ([0.0, 0.0, 0.0], [0.0, w, 0.0]),
        ([l, 0.0, h], [l, 0.0, 0.0]),
        ([l, 0.0, h], [l, w, h]),
        ([l, w, 0.0], [l, w, h]),
        ([l, 0.0, 0.0], [l, w, 0.0]),
        ([0.0, 0.0, 0.0], [l, 0.0, 0.0]),
        ([0.0, 0.0, h], [l, 0.0, h]),
        ([0.0, w, 0.0], [l, w, 0.0]),
        ([0.0, w, h], [l, w, h]),
    ];
    mesh.edge_points = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
    mesh.edge_point_counts = vec![2; 12];
    mesh
}

/// The design note's view (m57-part3-design.md, section 4): orthographic from (10, 10, 100)
/// onto (10, 10, 10), +y up, 40 mm high, 400 by 400 pixels. Right is +x and up +y, 10 pixels
/// a millimetre: (x, y) lands at column 200 + 10 (x − 10) and row 200 − 10 (y − 10), and a
/// point's depth from the eye is 100 − z.
fn from_above() -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 40.0 },
        eye: [10.0, 10.0, 100.0],
        target: [10.0, 10.0, 10.0],
        up: [0.0, 1.0, 0.0],
    }
}

const SIZE: [u32; 2] = [400, 400];

/// Where (x, y) lands seen [`from_above`], by hand.
fn above_px(x: f64, y: f64) -> [f64; 2] {
    [200.0 + 10.0 * (x - 10.0), 200.0 - 10.0 * (y - 10.0)]
}

/// What's under `pointer`, as (element, hidden), body 1 alone.
fn listed(hits: &[Hit]) -> Vec<(Element, bool)> {
    assert!(hits.iter().all(|hit| hit.body == 1), "{hits:?}");
    hits.iter().map(|hit| (hit.element, hit.hidden)).collect()
}

/// `hit` at `point` and `depth`, by hand, to a millionth of a millimetre: the renderer's
/// projection is its f32 matrix taken in f64 (`ray`, `project`), so a pixel's ray is where
/// the hand puts it to f32's precision across the view (here 7.5e-8 mm of a 10 mm offset).
#[track_caller]
fn assert_at(hit: &Hit, point: [f64; 3], depth: f64) {
    for k in 0..3 {
        assert!(
            (hit.point[k] - point[k]).abs() < 1e-6,
            "{hit:?} against {point:?}"
        );
    }
    assert!((hit.depth - depth).abs() < 1e-6, "{hit:?} against {depth}");
}

#[test]
fn select_others_order_on_a_box_seen_from_above_is_the_one_worked_by_hand() {
    use Element::{Edge, Face, Vertex};
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above();
    let bodies = [(1, &cube)];
    // The renderer's own projection lands where the hand says.
    let landed = project(&camera, SIZE, [19.8, 15.0, 20.0]).unwrap();
    assert!((landed[0] - 298.0).abs() < 1e-4 && (landed[1] - 150.0).abs() < 1e-4);
    // Over (19.8, 15), 2 pixels in from x = 20: the edges there, Edge6 (x = 20, z = 20) and
    // Edge8 (x = 20, z = 0), are 2 pixels off, within 6; no vertex within 7 (the nearest,
    // (20, 20), is 50 pixels off, and (20, 0) 150). The ray meets the top, Face6, at depth 80,
    // and the bottom, Face5, at 100; the sides x = 20 and y = 20 lie along it (seen edge
    // on). Each edge at its point nearest the pointer, (20, 15), 0.48 mm toward the eye
    // (1.2% of 40), so Edge6 at 79.52 before Face6 at 80; Edge8 and Face5 behind Face6.
    let under = hits(&camera, SIZE, 1.0, &bodies, above_px(19.8, 15.0));
    assert_eq!(
        listed(&under),
        [
            (Edge(6), false),
            (Face(6), false),
            (Edge(8), true),
            (Face(5), true)
        ]
    );
    assert_at(&under[0], [20.0, 15.0, 20.0], 80.0);
    assert_at(&under[1], [19.8, 15.0, 20.0], 80.0);
    assert_at(&under[2], [20.0, 15.0, 0.0], 100.0);
    assert_at(&under[3], [19.8, 15.0, 0.0], 100.0);
    // Over (15, 15), 50 pixels from every edge: the two faces alone.
    let under = hits(&camera, SIZE, 1.0, &bodies, above_px(15.0, 15.0));
    assert_eq!(listed(&under), [(Face(6), false), (Face(5), true)]);
    // Over (19.8, 19.8): the corner (20, 20, 20), Vertex7, 2.83 pixels off, first; then the
    // top's two edges there, Edge6 (x = 20) and Edge12 (y = 20), 2 pixels off, and Edge7,
    // the side's edge along z at (20, 20), seen end on (2.83 pixels off), at its end nearest
    // the eye: all at 80 less the reach, by number; then Face6. Behind it the bottom's:
    // Vertex8 (20, 20, 0), Edge8 and Edge11, then Face5.
    let under = hits(&camera, SIZE, 1.0, &bodies, above_px(19.8, 19.8));
    assert_eq!(
        listed(&under),
        [
            (Vertex(7), false),
            (Edge(6), false),
            (Edge(7), false),
            (Edge(12), false),
            (Face(6), false),
            (Vertex(8), true),
            (Edge(8), true),
            (Edge(11), true),
            (Face(5), true),
        ]
    );
    assert_at(&under[0], [20.0, 20.0, 20.0], 80.0);
    assert_at(&under[2], [20.0, 20.0, 20.0], 80.0);
    assert_at(&under[3], [19.8, 20.0, 20.0], 80.0);
    assert_at(&under[5], [20.0, 20.0, 0.0], 100.0);
}

#[test]
fn the_thresholds_are_the_prototypes_in_points_at_the_frames_pixels_per_point() {
    use Element::{Edge, Face, Vertex};
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above();
    let bodies = [(1, &cube)];
    let kinds = |under: &[Hit]| -> Vec<Element> { under.iter().map(|h| h.element).collect() };
    assert_eq!((HIT_EDGE_POINTS, HIT_VERTEX_POINTS), (6.0, 7.0));
    // An edge 5.9 pixels off is taken, 6 isn't (the prototype's `< 6`): over (19.41, 10) and
    // (19.4, 10), Edge6 at x = 20 is 5.9 and 6 pixels off.
    let near = hits(&camera, SIZE, 1.0, &bodies, above_px(19.41, 10.0));
    assert_eq!(kinds(&near), [Edge(6), Face(6), Edge(8), Face(5)]);
    let off = hits(&camera, SIZE, 1.0, &bodies, above_px(19.4, 10.0));
    assert_eq!(kinds(&off), [Face(6), Face(5)]);
    // At two pixels a point, 6 points are 12 pixels: 6 pixels off is taken.
    let doubled = hits(&camera, SIZE, 2.0, &bodies, above_px(19.4, 10.0));
    assert_eq!(kinds(&doubled), [Edge(6), Face(6), Edge(8), Face(5)]);
    // A vertex 6.9 pixels off is taken, 7 isn't: over the diagonal from (20, 20), 6.9 and 7
    // pixels along it (and 4.88 and 4.95 from each edge, so both edges are taken too).
    let along = |d: f64| {
        let off = d / 10.0 / 2f64.sqrt();
        above_px(20.0 - off, 20.0 - off)
    };
    let near = hits(&camera, SIZE, 1.0, &bodies, along(6.9));
    assert_eq!(near[0].element, Vertex(7));
    let off = hits(&camera, SIZE, 1.0, &bodies, along(7.0));
    assert!(!off.iter().any(|h| h.element == Vertex(7)), "{off:?}");
    assert_eq!(off[0].element, Edge(6));
    // At two pixels a point, 7 points are 14 pixels: the vertex 6.9 points (13.8 pixels) off
    // is taken, 7.1 (14.2) isn't; the edges, 9.76 and 10.04 pixels off, within 12, both times.
    let doubled = hits(&camera, SIZE, 2.0, &bodies, along(13.8));
    assert_eq!(doubled[0].element, Vertex(7), "{doubled:?}");
    let doubled = hits(&camera, SIZE, 2.0, &bodies, along(14.2));
    assert!(
        !doubled.iter().any(|h| h.element == Vertex(7)),
        "{doubled:?}"
    );
    assert_eq!(doubled[0].element, Edge(6));
}

#[test]
fn an_edge_or_a_vertex_exactly_at_its_threshold_isnt_taken_and_a_hair_inside_is() {
    use Element::{Edge, Face, Vertex};
    // The prototype's `< 6` and `< 7`, strictly. Seen from above, every point of the top's
    // edge at x = 20 lands in one column, X (an orthographic view's columns don't depend on
    // y or z), so a pointer at X − 6, on a row the edge spans, is 6 pixels from it exactly in
    // f64 (X and X − 6 share an exponent: the subtraction is exact both ways).
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above();
    let bodies = [(1, &cube)];
    let column = project(&camera, SIZE, [20.0, 15.0, 20.0]).unwrap()[0];
    let at = hits(&camera, SIZE, 1.0, &bodies, [column - 6.0, 150.0]);
    assert_eq!(listed(&at), [(Face(6), false), (Face(5), true)]);
    let inside = hits(&camera, SIZE, 1.0, &bodies, [column - 6.0 + 1e-9, 150.0]);
    assert_eq!(
        listed(&inside),
        [
            (Edge(6), false),
            (Face(6), false),
            (Edge(8), true),
            (Face(5), true)
        ]
    );
    // The corner (20, 20, 20), Vertex7, 7 pixels to its right exactly, on its row: on the
    // top's back edge (Edge12, y = 20), 7 pixels from the side's (Edge6, x = 20).
    let corner = project(&camera, SIZE, [20.0, 20.0, 20.0]).unwrap();
    let at = hits(&camera, SIZE, 1.0, &bodies, [corner[0] - 7.0, corner[1]]);
    assert!(!at.iter().any(|h| h.element == Vertex(7)), "{at:?}");
    assert!(!at.iter().any(|h| h.element == Edge(6)), "{at:?}");
    assert_eq!(at[0].element, Edge(12));
    let inside = hits(
        &camera,
        SIZE,
        1.0,
        &bodies,
        [corner[0] - 7.0 + 1e-9, corner[1]],
    );
    assert_eq!(inside[0].element, Vertex(7), "{inside:?}");
}

#[test]
fn an_edge_or_a_vertex_just_outside_the_silhouette_is_taken_through_the_bodys_grown_box() {
    use Element::{Edge, Vertex};
    // Over (20.3, 15), 3 pixels outside the top's edge at x = 20: the ray misses the box, so
    // no face is met, but the edges along x = 20 are 3 pixels off: Edge6 (z = 20) at 80 less
    // its reach, and Edge8 (z = 0) at 100 less it, hidden by the top, which the ray through
    // its own point, (20, 15), meets at its edge, at 80.
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above();
    let bodies = [(1, &cube)];
    let under = hits(&camera, SIZE, 1.0, &bodies, above_px(20.3, 15.0));
    assert_eq!(listed(&under), [(Edge(6), false), (Edge(8), true)]);
    assert_at(&under[0], [20.0, 15.0, 20.0], 80.0);
    assert_at(&under[1], [20.0, 15.0, 0.0], 100.0);
    // Over (20.3, 20.3), off the corner: Vertex7 (20, 20, 20) 4.24 pixels off, and the edges
    // that meet there, Edge6, Edge12, and Edge7 seen end on, as far; then the bottom's corner,
    // Vertex8, and its edges, Edge8 and Edge11, each hidden by the top's corner.
    let under = hits(&camera, SIZE, 1.0, &bodies, above_px(20.3, 20.3));
    assert_eq!(
        listed(&under),
        [
            (Vertex(7), false),
            (Edge(6), false),
            (Edge(7), false),
            (Edge(12), false),
            (Vertex(8), true),
            (Edge(8), true),
            (Edge(11), true),
        ]
    );
    assert_at(&under[0], [20.0, 20.0, 20.0], 80.0);
}

/// A face of four corners and two triangles, their corners' numbers among its own.
type Quad = ([[f64; 3]; 4], [[u32; 3]; 2]);

/// A square face, two triangles, `z` = `at` over x and y from `low` to `high`.
fn square_at(at: f64, low: f64, high: f64) -> Quad {
    (
        [
            [low, low, at],
            [high, low, at],
            [high, high, at],
            [low, high, at],
        ],
        [[0, 1, 2], [0, 2, 3]],
    )
}

/// A mesh of `faces`, each its corners and its triangles (their corners' numbers among its
/// own), numbered from 1 in order; no edges or vertices.
fn mesh_of(faces: &[Quad]) -> Mesh {
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for (corners, own) in faces {
        let first = u32::try_from(positions.len()).unwrap();
        positions.extend_from_slice(corners);
        triangles.extend(own.iter().map(|t| t.map(|c| c + first)));
    }
    Mesh {
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        positions,
        triangles,
        face_triangles: vec![2; faces.len()],
        valid: None,
        edge_points: vec![],
        edge_point_counts: vec![],
        vertex_points: vec![],
    }
}

#[test]
fn an_orthographic_vertexs_reach_is_at_most_halfway_to_the_near_plane_as_its_disc_is_drawn() {
    use Element::{Face, Vertex};
    // Zoomed out: from (10, 10, 100) onto (10, 10, 10), 4000 mm high over 400 rows, so 1.2%
    // of the view's height is 48 mm. Face1, z = 20, and Face2, z = 0, each over x and y from
    // −5 to 25; Vertex1 at (10, 10, 0), on Face2, under the pointer (the image's centre).
    // The renderer sets its camera past the bodies (their box's farthest corner from the
    // target, √(15² + 15² + 10²) = 23.4521 mm, and a millimetre: 24.4521 in front of the
    // target), so the vertex is 34.4521 mm in front of the near plane, and its disc is drawn
    // halfway there, 17.2260 mm toward the eye, not 48: at z = 17.226, under Face1. Its key
    // is 100 − 17.2260 = 82.7740, behind Face1 at 80, which hides it on its own ray. With
    // the 48 mm uncapped it would be listed first, at 52, and not hidden.
    let mut mesh = mesh_of(&[square_at(20.0, -5.0, 25.0), square_at(0.0, -5.0, 25.0)]);
    mesh.vertex_points = vec![[10.0, 10.0, 0.0]];
    let body = Pickable::new(Arc::new(mesh));
    let camera = Camera {
        projection: Projection::Orthographic {
            view_height: 4000.0,
        },
        ..from_above()
    };
    let under = hits(&camera, SIZE, 1.0, &[(1, &body)], [200.0, 200.0]);
    assert_eq!(
        listed(&under),
        [(Face(1), false), (Vertex(1), true), (Face(2), true)]
    );
    assert_at(&under[0], [10.0, 10.0, 20.0], 80.0);
    assert_at(&under[1], [10.0, 10.0, 0.0], 100.0);
    let ahead = 550f64.sqrt() + 1.0 + 10.0;
    assert!((ahead / 2.0 - 17.226_039).abs() < 1e-6, "{ahead}");
}

/// In perspective from the origin along −z, +y up, `field_of_view` high, 100 by 100 pixels:
/// at row 50 + r, the ray's y is −r / 50 · tan(fov / 2) of its depth.
fn from_the_origin_at(field_of_view: f64) -> Camera {
    Camera {
        projection: Projection::Perspective { field_of_view },
        ..from_the_origin()
    }
}

/// A floor tilted toward the eye, z = −10 − 0.3 y for y from −20 to 0 (x from −20 to 20),
/// Face1, and its far edge along x at (y, z) = (0, −10), Edge1, on the view's axis.
fn tilted_floor() -> Mesh {
    let z = |y: f64| -10.0 - 0.3 * y;
    let corners = [
        [-20.0, 0.0, z(0.0)],
        [20.0, 0.0, z(0.0)],
        [20.0, -20.0, z(-20.0)],
        [-20.0, -20.0, z(-20.0)],
    ];
    let mut mesh = mesh_of(&[(corners, [[0, 1, 2], [0, 2, 3]])]);
    mesh.edge_points = vec![[-20.0, 0.0, -10.0], [20.0, 0.0, -10.0]];
    mesh.edge_point_counts = vec![2];
    mesh
}

#[test]
fn a_perspective_faces_depth_off_the_axis_is_along_the_view_not_the_ray() {
    use Element::{Face, Vertex};
    // 90 degrees, the pointer at (90, 50), 38.66° off the axis (its ray's x 0.8 of its
    // depth): Face1, z = −10, met at (8, 0, −10), 12.806 along the ray but depth 10 along the
    // view. Vertex1 on the same ray 2 mm behind it, at (9.6, 0, −12), depth 12, less its reach
    // (1.2% of 2 tan 45° of its depth, 0.288) is 11.712: behind the face at 10, hidden. Taken
    // by the ray's 12.806, the face would come after the vertex, and not hide it.
    let mut mesh = mesh_of(&[square_at(-10.0, -20.0, 20.0)]);
    mesh.vertex_points = vec![[9.6, 0.0, -12.0]];
    let body = Pickable::new(Arc::new(mesh));
    let under = hits(
        &from_the_origin(),
        [100, 100],
        1.0,
        &[(1, &body)],
        [90.0, 50.0],
    );
    assert_eq!(listed(&under), [(Face(1), false), (Vertex(1), true)]);
    // Off the axis the ray's direction carries f32's rounding of the field of view: a part
    // in 10^7 of 8 mm.
    for k in 0..3 {
        assert!(
            (under[0].point[k] - [8.0, 0.0, -10.0][k]).abs() < 1e-5,
            "{under:?}"
        );
    }
    assert!((under[0].depth - 10.0).abs() < 1e-5, "{under:?}");
    assert!((under[1].depth - 12.0).abs() < 1e-9, "{under:?}");
}

#[test]
fn a_perspective_edges_reach_is_of_its_depth_and_at_most_a_quarter_of_it() {
    use Element::{Edge, Face};
    let floor = Pickable::new(Arc::new(tilted_floor()));
    let bodies = [(1, &floor)];
    let at = |camera: &Camera, row: f64| -> Vec<(Element, bool)> {
        listed(&hits(camera, [100, 100], 1.0, &bodies, [50.0, row]))
    };
    // 90 degrees: Edge1 at depth 10, its reach 1.2% of 2 tan 45° of that, 0.24, so its key
    // 9.76. At row 52 the ray (y −0.04 of its depth) meets the floor at depth s where s =
    // 10 − 0.3 · 0.04 s: 10 / 1.012 = 9.8814, behind the edge's key: the edge first. At row 55
    // (y −0.1) at 10 / 1.03 = 9.7087, before it: the floor first. Its reach not of its depth
    // (0.024), the floor would come first at row 52 too.
    let right = from_the_origin_at(90.0);
    assert_eq!(at(&right, 52.0), [(Edge(1), false), (Face(1), false)]);
    assert_eq!(at(&right, 55.0), [(Face(1), false), (Edge(1), false)]);
    // 170 degrees: 1.2% of 2 tan 85° is 0.2743, past the most, a quarter: the key 7.5. At row
    // 55 (y −0.1 tan 85° = −1.14301 of the depth) the floor is met at 10 / 1.342902 = 7.4466,
    // before it: the floor first (with 0.2743, the key 7.2568, the edge would be). At row 54
    // (y −0.914404) at 10 / 1.274321 = 7.8473, behind it: the edge first.
    let wide = from_the_origin_at(170.0);
    assert_eq!(at(&wide, 55.0), [(Face(1), false), (Edge(1), false)]);
    assert_eq!(at(&wide, 54.0), [(Edge(1), false), (Face(1), false)]);
    let tan = libm::tan(85f64.to_radians());
    assert!((VERTEX_REACH * 2.0 * tan - 0.274_321).abs() < 1e-6);
}

/// How many triangles the cast tests on this thread from now on.
fn tested_from_here() -> impl Fn() -> u64 {
    let start = super::TRIANGLES_TESTED.with(std::cell::Cell::get);
    move || super::TRIANGLES_TESTED.with(std::cell::Cell::get) - start
}

/// How many of a tree's boxes the cast tests on this thread from now on.
fn boxes_from_here() -> impl Fn() -> u64 {
    let start = super::BOXES_TESTED.with(std::cell::Cell::get);
    move || super::BOXES_TESTED.with(std::cell::Cell::get) - start
}

/// A face's corners and its triangles, their corners' numbers among its own.
type FaceParts = (Vec<[f64; 3]>, Vec<[u32; 3]>);

/// A face over (0, 0)–(20, 20) at `z`, `columns` by `rows` squares, two triangles a square,
/// row by row from y = 0 (each row from x = 0): so its chunks of 8 triangles are runs of 4
/// squares along a row. Its corners and triangles, numbered among its own.
fn grid(columns: u32, rows: u32, z: f64) -> FaceParts {
    let (dx, dy) = (20.0 / f64::from(columns), 20.0 / f64::from(rows));
    let positions = (0..=rows)
        .flat_map(|j| (0..=columns).map(move |i| [f64::from(i) * dx, f64::from(j) * dy, z]))
        .collect();
    let corner = |i: u32, j: u32| j * (columns + 1) + i;
    let triangles = (0..rows)
        .flat_map(|j| {
            (0..columns).flat_map(move |i| {
                [
                    [corner(i, j), corner(i + 1, j), corner(i + 1, j + 1)],
                    [corner(i, j), corner(i + 1, j + 1), corner(i, j + 1)],
                ]
            })
        })
        .collect();
    (positions, triangles)
}

/// A mesh of `faces` (each a [`grid`]'s corners and triangles), numbered from 1 in order, and
/// `vertex_points`; no edges.
fn faces_and_vertices(faces: Vec<FaceParts>, vertex_points: Vec<[f64; 3]>) -> Mesh {
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    let mut face_triangles = Vec::new();
    for (corners, own) in faces {
        let first = u32::try_from(positions.len()).unwrap();
        positions.extend(corners);
        face_triangles.push(u32::try_from(own.len()).unwrap());
        triangles.extend(own.iter().map(|t| t.map(|c| c + first)));
    }
    Mesh {
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        positions,
        triangles,
        face_triangles,
        valid: None,
        edge_points: vec![],
        edge_point_counts: vec![],
        vertex_points,
    }
}

/// Where the tree tests below cast, (x, y) = (15.17578125, 14.921875), exact in binary: well
/// inside a square, a chunk, and every box of the grids they cast on, so no rounding puts
/// the line on a side. On a [`grid`] of 256 by 128 squares (0.078125 by 0.15625 mm), a
/// quarter of the way across square 194 of row 95, in the run of squares 192 to 195 (x from
/// 15 to 15.3125), 0.0781 mm from the row's sides; on one of 64 by 32 (0.3125 by 0.625 mm),
/// in square 48 of row 23, the run 48 to 51 (x 15 to 16.25), 0.078 mm below the row's top.
const ON_A_CHUNK: [f64; 2] = [15.175_781_25, 14.921_875];

#[test]
fn a_crowd_hidden_behind_a_face_of_sixty_five_thousand_triangles_is_cast_on_a_few_of_its_boxes() {
    use Element::{Face, Vertex};
    // A grid of 256 by 128 squares at z = 20, one face of 65,536 triangles in 8,192 chunks
    // (2^13: 64 runs of 4 squares across, 128 rows), seen from above at `ON_A_CHUNK`. Each
    // split halves a block of whole runs or whole rows along the way its centres spread most
    // (rows, runs, rows, ...: 128 rows to 64 is a spread of 19.84 against 19.69), so each
    // node's box is its block's and none overlaps another of its level; 11 splits make
    // leaves of 4 chunks (8,192 / 2^11), a 2 by 2 block. A line through the one chunk meets
    // one box of each level: the cast tests the root, both children of each of the 11 nodes
    // on its way, and the leaf's 4 chunks, 1 + 22 + 4 = 27 boxes (twice log2 of the chunks'
    // number, and one), and the 8 triangles of its chunk alone. In one leaf of all 8,192
    // (no tree), 8,193; past the log, the tree's whole row.
    let (x, y) = (ON_A_CHUNK[0], ON_A_CHUNK[1]);
    let pointer = above_px(x, y);
    let alone = Pickable::new(Arc::new(faces_and_vertices(
        vec![grid(256, 128, 20.0)],
        vec![],
    )));
    let (boxes, tested) = (boxes_from_here(), tested_from_here());
    let under = hits(&from_above(), SIZE, 1.0, &[(1, &alone)], pointer);
    assert_eq!(listed(&under), [(Face(1), false)]);
    assert_at(&under[0], [x, y, 20.0], 80.0);
    assert_eq!((boxes(), tested()), (27, 8));
    // A thousand vertices under it at (x, y, 0): the face first, at 80, then the nearest 255
    // vertices (at 100 less their reach), each hidden by the face. Each of those 255 casts,
    // through its vertex, walks the same way, but stops at the first triangle met nearer
    // (the face's, at 80): at most the 27 boxes and the 8 triangles the pointer's cast tests,
    // so 256 walks at most 256 · 27 = 6,912 boxes and 2,048 triangles; not the face's whole
    // row of 65,536 triangles each (16.8 million, cast face by face), nor every chunk's box
    // (2 million, with no tree).
    let crowd = Pickable::new(Arc::new(faces_and_vertices(
        vec![grid(256, 128, 20.0)],
        vec![[x, y, 0.0]; 1000],
    )));
    let (boxes, tested) = (boxes_from_here(), tested_from_here());
    let under = hits(&from_above(), SIZE, 1.0, &[(1, &crowd)], pointer);
    let mut want = vec![(Face(1), false)];
    want.extend((1..=255).map(|n| (Vertex(n), true)));
    assert_eq!(listed(&under), want);
    let (boxes, tested) = (boxes(), tested());
    assert!(boxes <= 256 * 27, "{boxes} boxes tested");
    assert!(tested <= 256 * 8, "{tested} triangles tested");
}

#[test]
fn vertices_in_front_of_a_stack_of_faces_are_judged_unhidden_by_its_root_box_alone() {
    use Element::{Face, Vertex};
    // Four grids of 64 by 32 squares at z = 20, 19, 18, and 17 (Face1 to Face4, 512 chunks
    // each, 2,048 in all), and 252 vertices at (x, y, 25) in front of them, seen from above
    // at `ON_A_CHUNK`: the vertices first (75 less their reach, 0.48), none hidden, then the
    // top face (80), and the three under it, hidden: 256, the most. Whether a face hides a
    // vertex is asked of the tree only nearer the eye than the vertex's 74.52, and its root's
    // box is no nearer than the top face's 80: so each of the 252 casts tests the root's box
    // alone, and goes no farther. The pointer's cast finds every face, so walks down to each
    // layer's chunk on its line. Its blocks (runs by rows by layers, 16 by 32 by 4) halve
    // along the way their centres spread most: rows, runs, rows, runs, rows, runs to 2 by 4
    // by 4 (32 chunks), whose layers spread most (3 mm, the rows 1.875), then the layers,
    // then rows and runs to leaves of a run by 2 rows by 2 layers. So the line is in one box
    // of each of the first 7 levels and in two of each after: the root, both children of
    // each of the 7 nodes on its way, both of each of the 2 nodes at each of the next two
    // levels, and the 4 chunks of each of its 2 leaves: 1 + 14 + 4 + 4 + 8 = 31 boxes, and
    // the 8 triangles of each of the 4 chunks it passes; 31 + 252 = 283 boxes in all. Asked
    // of every box (no depth prune), each vertex's walk would go down all four faces as the
    // pointer's does, 253 · 31 = 7,843; in one leaf (no tree), 2,049 + 252.
    let (x, y) = (ON_A_CHUNK[0], ON_A_CHUNK[1]);
    let stack = Pickable::new(Arc::new(faces_and_vertices(
        [20.0, 19.0, 18.0, 17.0]
            .map(|z| grid(64, 32, z))
            .into_iter()
            .collect(),
        vec![[x, y, 25.0]; 252],
    )));
    let (boxes, tested) = (boxes_from_here(), tested_from_here());
    let under = hits(&from_above(), SIZE, 1.0, &[(1, &stack)], above_px(x, y));
    let mut want: Vec<(Element, bool)> = (1..=252).map(|n| (Vertex(n), false)).collect();
    want.extend([
        (Face(1), false),
        (Face(2), true),
        (Face(3), true),
        (Face(4), true),
    ]);
    assert_eq!(listed(&under), want);
    for (hit, depth) in under[252..].iter().zip([80.0, 81.0, 82.0, 83.0]) {
        assert_at(hit, [x, y, 100.0 - depth], depth);
    }
    assert_eq!((boxes(), tested()), (31 + 252, 4 * 8));
}

#[test]
fn triangles_no_line_can_meet_are_left_out_of_the_tree_so_a_crowd_behind_them_costs_no_box() {
    use Element::Vertex;
    // 100,002 triangles of one face on the pointer's line, none of any area: a third at one
    // point, (x, y, 20); a third with their first two corners at (0, 0, 20) and the third at
    // (20, 20, 20); a third with their first and last there. Each has its first corner
    // repeated, so no line meets it (`Line::meets` divides by an exact 0), and it draws
    // nothing; but in the tree, each chunk's box would hold the line, (0, 0)–(20, 20), so
    // each of the 257 walks would test all 100,002 triangles (25.7 million) and every box.
    // Left out, the tree is empty: no box, no triangle. 300 vertices behind them at (x, y,
    // 0): the nearest 256, by number, none hidden, as nothing is met.
    let (x, y) = (ON_A_CHUNK[0], ON_A_CHUNK[1]);
    let mesh = Mesh {
        positions: vec![[x, y, 20.0], [0.0, 0.0, 20.0], [20.0, 20.0, 20.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        triangles: [[0, 0, 0], [1, 1, 2], [1, 2, 1]].repeat(33_334),
        face_triangles: vec![100_002],
        valid: None,
        edge_points: vec![],
        edge_point_counts: vec![],
        vertex_points: vec![[x, y, 0.0]; 300],
    };
    let none = Pickable::new(Arc::new(mesh));
    let (boxes, tested) = (boxes_from_here(), tested_from_here());
    let under = hits(&from_above(), SIZE, 1.0, &[(1, &none)], above_px(x, y));
    let want: Vec<(Element, bool)> = (1..=256).map(|n| (Vertex(n), false)).collect();
    assert_eq!(listed(&under), want);
    assert_eq!((boxes(), tested()), (0, 0));
}

#[test]
fn a_node_left_unfilled_is_walked_as_nothing_never_round_for_ever() {
    // A tree whose root `Tree::of` left as it was before filling it (never, but a range that
    // can't be would): walked, its box holding the line, it's a leaf of no chunks, so the
    // walk tests its box once and ends, meeting nothing. Were it a node of children (the
    // root's own place among them), the walk would go round for ever.
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above();
    let bodies = [(1, &cube)];
    let caster = super::Caster::new(&camera, SIZE, 1.0, &bodies).unwrap();
    let (origin, direction) = ray(&camera, SIZE, above_px(15.0, 15.0)).unwrap();
    let line = super::Line {
        origin,
        direction,
        whole: true,
    };
    let tree = super::Tree {
        nodes: vec![super::TreeNode::unfilled(([0.0; 3], [20.0; 3]))],
        chunks: vec![],
    };
    let asked = std::cell::Cell::new(0u32);
    let boxes = boxes_from_here();
    let met = tree.walk(
        &caster,
        cube.mesh(),
        &line,
        |_| {
            asked.set(asked.get() + 1);
            assert!(asked.get() < 1000, "the walk goes round");
            true
        },
        |_, _, _| true,
    );
    assert!(!met);
    assert_eq!((asked.get(), boxes()), (1, 1));
}

/// Orthographic from above the box's front, looking down at 45 degrees: from (10, −90, 110)
/// onto (10, 10, 10), +z up, 40 mm high, 400 by 400 pixels. Forward is (0, 1, −1)/√2, right
/// +x, and up (0, 1, 1)/√2; a point's depth from the eye is (y − z + 200)/√2, and its row
/// 200 − 10 (y + z − 20)/√2. The top face (z = 20) recedes upward in the image: its back
/// edge, Edge12 (y = 20, z = 20), lands at row 200 − 200/√2 = 58.58, at depth 200/√2 =
/// 141.42.
fn from_above_the_front() -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 40.0 },
        eye: [10.0, -90.0, 110.0],
        target: [10.0, 10.0, 10.0],
        up: [0.0, 0.0, 1.0],
    }
}

#[test]
fn an_edge_on_the_face_under_the_pointer_is_never_hidden_by_it_and_comes_first_within_its_reach() {
    use Element::{Edge, Face};
    // Departure from the design note's rule (no tolerance: an edge hidden where a face is
    // met strictly nearer): the edge's point is beside the pointer, not on its ray, and here
    // the face under the pointer is nearer than the edge by what the pointer is off it.
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above_the_front();
    let bodies = [(1, &cube)];
    let row = 200.0 - 200.0 / 2f64.sqrt();
    let landed = project(&camera, SIZE, [10.0, 20.0, 20.0]).unwrap();
    assert!((landed[0] - 200.0).abs() < 1e-4 && (landed[1] - row).abs() < 1e-4);
    // 2 pixels below the edge, 0.2 mm down the image: the top face met at y = 20 − 0.2√2,
    // depth 141.42 − 0.2 = 141.22; Edge12 at (10, 20, 20), 141.42, less its reach 0.48 is
    // 140.94: first, and on the ray through its own point the top face is met at its own
    // depth, so not hidden. Beyond the top the ray leaves through the back face, Face4
    // (y = 20), at z = 20 − 0.2√2, depth 141.42 + 0.2 = 141.62: hidden.
    let under = hits(&camera, SIZE, 1.0, &bodies, [200.0, row + 2.0]);
    assert_eq!(
        listed(&under),
        [(Edge(12), false), (Face(6), false), (Face(4), true)]
    );
    let a = 200.0 / 2f64.sqrt();
    let off = 0.2 * 2f64.sqrt();
    assert_at(&under[0], [10.0, 20.0, 20.0], a);
    assert_at(&under[1], [10.0, 20.0 - off, 20.0], a - 0.2);
    assert_at(&under[2], [10.0, 20.0, 20.0 - off], a + 0.2);
    // 5 pixels below, 0.5 mm, past the reach: the edge after the face, still not hidden.
    let under = hits(&camera, SIZE, 1.0, &bodies, [200.0, row + 5.0]);
    assert_eq!(
        listed(&under),
        [(Face(6), false), (Edge(12), false), (Face(4), true)]
    );
    assert!((VERTEX_REACH * 40.0 - 0.48).abs() < 1e-12);
}

/// In perspective from the origin along −z, +y up, 90 degrees high, 100 by 100 pixels: (x, y,
/// z) lands at column 50 + 50 x / −z and row 50 − 50 y / −z, its depth −z. The pointer at
/// the image's centre, (50, 50).
fn from_the_origin() -> Camera {
    Camera {
        projection: Projection::Perspective {
            field_of_view: 90.0,
        },
        eye: [0.0; 3],
        target: [0.0, 0.0, -10.0],
        up: [0.0, 1.0, 0.0],
    }
}

/// Two triangles across the view's axis, each a face: Face1 at depth `near`, Face2 at 2.
fn two_faces(near: f64) -> Mesh {
    let at = |z: f64| [[-1.0, -1.0, z], [1.0, -1.0, z], [0.0, 2.0, z]];
    let positions: Vec<[f64; 3]> = at(-near).into_iter().chain(at(-2.0)).collect();
    Mesh {
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        positions,
        triangles: vec![[0, 1, 2], [3, 4, 5]],
        face_triangles: vec![1, 1],
        valid: None,
        edge_points: vec![],
        edge_point_counts: vec![],
        vertex_points: vec![],
    }
}

#[test]
fn in_perspective_nothing_nearer_the_eye_than_the_drawing_cuts_is_met() {
    use Element::{Edge, Face, Vertex};
    let mut mesh = two_faces(MIN_EYE_DISTANCE / 2.0);
    // Edge1 from behind the eye, (0.5, 0, 10), to (0.5, 0, −10): drawn from depth 1 nm on,
    // landing from far right to column 52.5 at its far end, 2.5 pixels off the pointer.
    // Edge2 wholly behind the eye, (−1, 0, 10) to (1, 0, 10): drawn nowhere, though taken
    // through the projection as it is (w = −10) it would cross the image's centre.
    mesh.edge_points = vec![
        [0.5, 0.0, 10.0],
        [0.5, 0.0, -10.0],
        [-1.0, 0.0, 10.0],
        [1.0, 0.0, 10.0],
    ];
    mesh.edge_point_counts = vec![2, 2];
    // Vertex1 behind the eye, on its axis; Vertex2 on Face2, 1 pixel off the pointer.
    mesh.vertex_points = vec![[0.0, 0.0, 10.0], [0.04, 0.0, -2.0]];
    let body = Pickable::new(Arc::new(mesh));
    let camera = from_the_origin();
    let under = hits(&camera, [100, 100], 1.0, &[(1, &body)], [50.0, 50.0]);
    // Face1, half a nanometre from the eye, isn't drawn, so isn't met, nor does it hide.
    // Vertex2 lies on Face2: its reach toward the eye (1.2% of 2 tan 45° = 0.024 of its
    // depth) puts it first, and Face2 is met at its own depth on its ray, so it's not hidden.
    // Edge1 at its far end, depth 10, behind Face2 on the ray through it.
    assert_eq!(
        listed(&under),
        [(Vertex(2), false), (Face(2), false), (Edge(1), true)]
    );
    assert_at(&under[0], [0.04, 0.0, -2.0], 2.0);
    assert_at(&under[1], [0.0, 0.0, -2.0], 2.0);
    assert_at(&under[2], [0.5, 0.0, -10.0], 10.0);
    // Face1 two nanometres off is drawn, met first, and hides what's behind it.
    let mut mesh = two_faces(2.0 * MIN_EYE_DISTANCE);
    mesh.vertex_points = vec![[0.04, 0.0, -2.0]];
    let body = Pickable::new(Arc::new(mesh));
    let under = hits(&camera, [100, 100], 1.0, &[(1, &body)], [50.0, 50.0]);
    assert_eq!(
        listed(&under),
        [(Face(1), false), (Vertex(1), true), (Face(2), true)]
    );
}

#[test]
fn bodies_at_one_depth_are_ordered_by_kind_then_body_then_number() {
    use Element::{Edge, Face};
    // Two copies of the box in one place, bodies 2 and 7, given 7 first: each face and edge
    // twice, at one depth, body 2's first.
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above();
    let under = hits(
        &camera,
        SIZE,
        1.0,
        &[(7, &cube), (2, &cube)],
        above_px(19.8, 15.0),
    );
    let got: Vec<(u32, Element, bool)> = under
        .iter()
        .map(|h| (h.body, h.element, h.hidden))
        .collect();
    assert_eq!(
        got,
        [
            (2, Edge(6), false),
            (7, Edge(6), false),
            (2, Face(6), false),
            (7, Face(6), false),
            (2, Edge(8), true),
            (7, Edge(8), true),
            (2, Face(5), true),
            (7, Face(5), true),
        ]
    );
}

#[test]
fn hostile_casts_find_nothing_or_what_they_can_and_never_panic() {
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let camera = from_above();
    let bodies = [(1, &cube)];
    let centre = above_px(15.0, 15.0);
    // A pointer outside the image, or not finite: nothing is drawn there.
    for pointer in [
        [-0.5, 200.0],
        [200.0, -0.5],
        [400.0, 200.0],
        [200.0, 400.0],
        [f64::NAN, 200.0],
        [200.0, f64::INFINITY],
        [f64::MAX, f64::MIN],
    ] {
        assert!(
            hits(&camera, SIZE, 1.0, &bodies, pointer).is_empty(),
            "{pointer:?}"
        );
    }
    // Pixels a point the frame refuses, a size of 0, and cameras the checks refuse.
    for ppp in [0.0, -1.0, f32::NAN, f32::INFINITY, 32.5] {
        assert!(
            hits(&camera, SIZE, ppp, &bodies, centre).is_empty(),
            "{ppp}"
        );
    }
    assert!(hits(&camera, [0, 400], 1.0, &bodies, centre).is_empty());
    let refused = [
        Camera {
            eye: camera.target,
            ..camera.clone()
        },
        Camera {
            up: [0.0, 0.0, 1.0],
            ..camera.clone()
        },
        Camera {
            projection: Projection::Orthographic { view_height: 0.0 },
            ..camera.clone()
        },
        Camera {
            projection: Projection::Orthographic {
                view_height: f64::NAN,
            },
            ..camera.clone()
        },
        Camera {
            projection: Projection::Perspective { field_of_view: 0.0 },
            ..camera.clone()
        },
        Camera {
            eye: [f64::INFINITY, 0.0, 0.0],
            ..camera.clone()
        },
    ];
    for camera in &refused {
        assert!(
            hits(camera, SIZE, 1.0, &bodies, centre).is_empty(),
            "{camera:?}"
        );
    }
    // No bodies, and an empty mesh: nothing.
    assert!(hits(&camera, SIZE, 1.0, &[], centre).is_empty());
    let empty = Pickable::new(Arc::new(Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles: vec![],
        valid: None,
        edge_points: vec![],
        edge_point_counts: vec![],
        vertex_points: vec![],
    }));
    assert!(hits(&camera, SIZE, 1.0, &[(1, &empty)], centre).is_empty());
    // A mesh that doesn't hold together (a triangle past its positions, faces counting more
    // triangles than it has, edges more points): what's whole is met, the rest passed over.
    let mut broken = freecad_box(20.0, 20.0, 20.0);
    broken.triangles[10] = [0, 1, u32::MAX];
    broken.face_triangles.push(u32::MAX);
    broken.edge_point_counts.push(u32::MAX);
    let broken = Pickable::new(Arc::new(broken));
    // (15, 15) is on the diagonal Face6's two triangles share: its first, broken, is passed
    // over, its second meets the ray at its edge; the seventh face and the thirteenth edge,
    // past what the mesh holds, are nothing.
    let under = hits(&camera, SIZE, 1.0, &[(1, &broken)], centre);
    assert_eq!(
        listed(&under),
        [(Element::Face(6), false), (Element::Face(5), true)]
    );
    // Its broken triangle alone under (15, 5), clear of the diagonal: Face6 isn't met.
    let under = hits(&camera, SIZE, 1.0, &[(1, &broken)], above_px(15.0, 5.0));
    assert_eq!(listed(&under), [(Element::Face(5), false)]);
}

#[test]
fn a_box_as_far_out_as_the_renderer_draws_is_met_as_one_at_the_origin_is() {
    use Element::{Edge, Face};
    // The box moved out to MAX_RENDER_COORDINATE on x and y (its far side and the camera
    // just inside it, as the checks take them), seen from above as before: the same list.
    // Its points are still exact (1e12 − 80 is an f64), the cast's rounding a part in 2^52
    // of 1e12, 0.2 µm, far inside the 2 pixels (0.2 mm) the edge is off.
    let far = MAX_RENDER_COORDINATE - 100.0;
    let mut mesh = freecad_box(20.0, 20.0, 20.0);
    let moved = |p: &[f64; 3]| [p[0] + far, p[1] + far, p[2]];
    mesh.positions = mesh.positions.iter().map(moved).collect();
    mesh.edge_points = mesh.edge_points.iter().map(moved).collect();
    mesh.vertex_points = mesh.vertex_points.iter().map(moved).collect();
    let cube = Pickable::new(Arc::new(mesh));
    let camera = Camera {
        eye: [10.0 + far, 10.0 + far, 100.0],
        target: [10.0 + far, 10.0 + far, 10.0],
        ..from_above()
    };
    let under = hits(&camera, SIZE, 1.0, &[(1, &cube)], above_px(19.8, 15.0));
    assert_eq!(
        listed(&under),
        [
            (Edge(6), false),
            (Face(6), false),
            (Edge(8), true),
            (Face(5), true)
        ]
    );
    // Past the renderer's coordinates, out to f64's largest: nothing panics, and nothing
    // met is anything but finite.
    let mut huge = freecad_box(20.0, 20.0, 20.0);
    let huge_by = |p: &[f64; 3]| p.map(|c| if c == 0.0 { -f64::MAX } else { f64::MAX });
    huge.positions = huge.positions.iter().map(huge_by).collect();
    huge.edge_points = huge.edge_points.iter().map(huge_by).collect();
    huge.vertex_points = huge.vertex_points.iter().map(huge_by).collect();
    let huge = Pickable::new(Arc::new(huge));
    for camera in [from_above(), from_the_origin()] {
        let under = hits(&camera, SIZE, 1.0, &[(1, &huge)], above_px(15.0, 15.0));
        assert!(
            under
                .iter()
                .all(|h| h.depth.is_finite() && h.point.iter().all(|c| c.is_finite())),
            "{under:?}"
        );
    }
}

#[test]
fn a_crowd_under_one_pixel_lists_its_nearest_few_hundred_by_number() {
    use Element::{Face, Vertex};
    // Ten thousand vertices at the top's point (15, 15, 20), and as many at the bottom's
    // (15, 15, 0): the nearest 256, those on the top (79.52), by number, none hidden (the
    // top is met at their own depth); the bottom's, and the faces, past them.
    let mut mesh = freecad_box(20.0, 20.0, 20.0);
    mesh.vertex_points = std::iter::repeat_n([15.0, 15.0, 20.0], 10_000)
        .chain(std::iter::repeat_n([15.0, 15.0, 0.0], 10_000))
        .collect();
    let crowd = Pickable::new(Arc::new(mesh));
    let under = hits(
        &from_above(),
        SIZE,
        1.0,
        &[(1, &crowd)],
        above_px(15.0, 15.0),
    );
    assert_eq!(MAX_HITS, 256);
    let want: Vec<(Element, bool)> = (1..=256).map(|n| (Vertex(n), false)).collect();
    assert_eq!(listed(&under), want);
    assert!(!under.iter().any(|h| h.element == Face(6)));
}

#[test]
fn a_cast_is_the_same_each_time_and_for_its_bodies_in_any_order() {
    // Deterministic: the same list, bit for bit, however the bodies are given.
    let cube = Pickable::new(Arc::new(freecad_box(20.0, 20.0, 20.0)));
    let other = Pickable::new(Arc::new(box_mesh([5.0, 5.0, -5.0], [10.0, 10.0, 40.0])));
    let camera = from_above_the_front();
    for pointer in [[200.0, 60.0], [150.5, 120.25], [260.0, 200.0]] {
        let one = hits(&camera, SIZE, 1.0, &[(1, &cube), (2, &other)], pointer);
        let two = hits(&camera, SIZE, 1.0, &[(2, &other), (1, &cube)], pointer);
        assert_eq!(one, two, "{pointer:?}");
        assert!(!one.is_empty(), "{pointer:?}");
        for pair in one.windows(2) {
            assert!(pair[0] != pair[1], "{one:?}");
        }
    }
}

/// A coordinate biased to the edges: 0, tiny, unit, large, as far as the renderer draws, and
/// anything between.
fn coordinate() -> impl proptest::strategy::Strategy<Value = f64> {
    use proptest::prelude::*;
    prop_oneof![
        Just(0.0),
        Just(1e-9),
        Just(-1e-9),
        Just(MAX_RENDER_COORDINATE),
        Just(-MAX_RENDER_COORDINATE),
        -1e3..1e3f64,
        -1e9..1e9f64,
    ]
}

/// A side of a box: tiny, unit, or large.
fn side() -> impl proptest::strategy::Strategy<Value = f64> {
    use proptest::prelude::*;
    prop_oneof![Just(1e-6), Just(1.0), 1e-3..1e3f64, Just(1e6)]
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 1000, ..Default::default() })]

    /// Whatever the box, camera, and pointer (most of them looking nowhere near the box: the
    /// hostile cases): no panic, the same list each time, each hit one of the box's
    /// elements, at a finite point, once; each face's point on the pointer's pixel and each
    /// edge's and vertex's within its threshold of it, by the renderer's own projection
    /// (`project`); the faces nearest first, the nearest of them alone unhidden (those as
    /// near too). What's found is checked, both ways, by the next test, whose cameras look
    /// at the box.
    #[test]
    fn any_cast_is_sane_and_agrees_with_the_projection(
        low in [coordinate(), coordinate(), coordinate()],
        size in [side(), side(), side()],
        eye in [coordinate(), coordinate(), coordinate()],
        towards in [-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64],
        distance in proptest::prop_oneof![1e-3..1.0f64, 1.0..1e3f64, 1e3..1e8f64],
        perspective in proptest::bool::ANY,
        across in proptest::prop_oneof![1e-3..1.0f64, 1.0..1e4f64, 0.5..179.5f64],
        pointer in [0.0..64.0f64, 0.0..48.0f64],
        ppp in proptest::prop_oneof![proptest::strategy::Just(1.0f32), 0.5..4.0f32],
    ) {
        let mesh = freecad_box(size[0], size[1], size[2]);
        let moved = |p: &[f64; 3]| std::array::from_fn(|k| p[k] + low[k]);
        let mesh = Mesh {
            positions: mesh.positions.iter().map(moved).collect(),
            edge_points: mesh.edge_points.iter().map(moved).collect(),
            vertex_points: mesh.vertex_points.iter().map(moved).collect(),
            ..mesh
        };
        let body = Pickable::new(Arc::new(mesh.clone()));
        let target = std::array::from_fn(|k| eye[k] + towards[k] * distance);
        let camera = Camera {
            projection: if perspective {
                Projection::Perspective { field_of_view: across.clamp(0.5, 179.5) }
            } else {
                Projection::Orthographic { view_height: across }
            },
            eye,
            target,
            up: [0.0, 0.0, 1.0],
        };
        let size_px = [64, 48];
        let under = hits(&camera, size_px, ppp, &[(1, &body)], pointer);
        proptest::prop_assert_eq!(&under, &hits(&camera, size_px, ppp, &[(1, &body)], pointer));
        let mut seen = std::collections::BTreeSet::new();
        for hit in &under {
            proptest::prop_assert!(seen.insert(hit.element), "{:?} twice", hit.element);
            let (n, count) = match hit.element {
                Element::Face(n) => (n, 6),
                Element::Edge(n) => (n, 12),
                Element::Vertex(n) => (n, 8),
            };
            proptest::prop_assert!((1..=count).contains(&n), "{:?}", hit);
            proptest::prop_assert!(hit.depth.is_finite() && hit.point.iter().all(|c| c.is_finite()));
            if perspective {
                proptest::prop_assert!(hit.depth > 0.0, "{:?}", hit);
            }
            // Where its point lands, by the renderer's projection: on the pointer's pixel for
            // a face, within the threshold for an edge or a vertex; give or take the point's
            // own rounding (a hundred parts in 2^52 of its largest coordinate, or the eye's),
            // in pixels at its depth.
            if let Some(at) = project(&camera, size_px, hit.point) {
                let (dx, dy) = (at[0] - pointer[0], at[1] - pointer[1]);
                let off = (dx * dx + dy * dy).sqrt();
                let scale = hit.point.iter().chain(&eye).map(|c| c.abs()).fold(1.0, f64::max);
                let pixel_mm = if perspective {
                    2.0 * libm::tan(across.clamp(0.5, 179.5).to_radians() / 2.0) * hit.depth / 48.0
                } else {
                    across / 48.0
                };
                let slack = 1e-3 + scale * 1e-14 / pixel_mm;
                let within = match hit.element {
                    Element::Face(_) => slack,
                    Element::Edge(_) => HIT_EDGE_POINTS * f64::from(ppp) + slack,
                    Element::Vertex(_) => HIT_VERTEX_POINTS * f64::from(ppp) + slack,
                };
                proptest::prop_assert!(off <= within, "{:?} lands {} off, within {}", hit, off, within);
            }
        }
        let faces: Vec<&Hit> = under.iter().filter(|h| matches!(h.element, Element::Face(_))).collect();
        for pair in faces.windows(2) {
            proptest::prop_assert!(pair[0].depth <= pair[1].depth, "{:?}", faces);
        }
        if let Some(first) = faces.first() {
            for face in &faces {
                proptest::prop_assert_eq!(face.hidden, face.depth > first.depth, "{:?}", face);
            }
        }
    }

    /// A camera looking at the box from outside it (its target in the box, its eye 1.5 to 20
    /// of the box's diagonal away, the box filling 0.3 to 4 of the view's height), the
    /// pointer within 10 pixels of a point of the box as it lands: each hit within its
    /// threshold of the pointer, as before; and the reverse, every vertex `project` lands
    /// within 7 points of the pointer listed, and every edge whose ends' landings make a
    /// segment within 6 points of it (the whole box is in front of the eye, so its edges
    /// are drawn whole). The faces by the box's own geometry, worked here, not by the cast:
    /// a line meets a box (slab by slab) entering through one face and leaving through
    /// another, so the faces listed are those two, the one it enters first and not hidden,
    /// the one it leaves hidden; none where the line clearly misses the box. A line within
    /// rounding of an edge or a corner of the box, where it meets more, is left out of that.
    #[test]
    fn a_cast_at_a_box_lists_whats_near_the_pointer_both_ways_and_the_faces_the_ray_crosses(
        low in [offset(), offset(), offset()],
        size in [resolved_side(), resolved_side(), resolved_side()],
        aim in [0.1..0.9f64, 0.1..0.9f64, 0.1..0.9f64],
        from in [-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64],
        away in 1.5..20.0f64,
        perspective in proptest::bool::ANY,
        fill in 0.3..4.0f64,
        seen in [0.0..1.0f64, 0.0..1.0f64, 0.0..1.0f64],
        nudge in [-10.0..10.0f64, -10.0..10.0f64],
        ppp in proptest::prop_oneof![proptest::strategy::Just(1.0f32), 0.5..4.0f32],
    ) {
        let norm = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let length = norm(from);
        if length < 0.1 {
            return Ok(());
        }
        let toward: [f64; 3] = from.map(|c| c / length);
        let mesh = freecad_box(size[0], size[1], size[2]);
        let moved = |p: &[f64; 3]| std::array::from_fn(|k| p[k] + low[k]);
        let mesh = Mesh {
            positions: mesh.positions.iter().map(moved).collect(),
            edge_points: mesh.edge_points.iter().map(moved).collect(),
            vertex_points: mesh.vertex_points.iter().map(moved).collect(),
            ..mesh
        };
        let body = Pickable::new(Arc::new(mesh.clone()));
        let diagonal = norm(size);
        let target: [f64; 3] = std::array::from_fn(|k| low[k] + aim[k] * size[k]);
        let eye: [f64; 3] = std::array::from_fn(|k| target[k] + toward[k] * away * diagonal);
        let up = if toward[2].abs() > 0.9 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
        let height = diagonal / fill;
        let camera = Camera {
            projection: if perspective {
                let angle = 2.0 * libm::atan(height / 2.0 / (away * diagonal));
                Projection::Perspective { field_of_view: angle.to_degrees().clamp(1.0, 170.0) }
            } else {
                Projection::Orthographic { view_height: height }
            },
            eye,
            target,
            up,
        };
        let size_px = [64u32, 48u32];
        let point: [f64; 3] = std::array::from_fn(|k| low[k] + seen[k] * size[k]);
        let Some(lands) = project(&camera, size_px, point) else {
            return Ok(());
        };
        let pointer = [
            (lands[0] + nudge[0]).clamp(0.0, 63.999),
            (lands[1] + nudge[1]).clamp(0.0, 47.999),
        ];
        let under = hits(&camera, size_px, ppp, &[(1, &body)], pointer);
        proptest::prop_assert_eq!(&under, &hits(&camera, size_px, ppp, &[(1, &body)], pointer));
        let ppp = f64::from(ppp);
        let off = |at: [f64; 2]| {
            let (dx, dy) = (at[0] - pointer[0], at[1] - pointer[1]);
            (dx * dx + dy * dy).sqrt()
        };
        for hit in &under {
            proptest::prop_assert!(hit.depth.is_finite() && hit.point.iter().all(|c| c.is_finite()));
            // Give or take the point's own rounding (a hundred parts in 2^52 of its largest
            // coordinate, or the eye's), in pixels at its depth.
            let scale = hit.point.iter().chain(&eye).map(|c| c.abs()).fold(1.0, f64::max);
            let pixel_mm = match camera.projection {
                Projection::Perspective { field_of_view } => {
                    2.0 * libm::tan(field_of_view.to_radians() / 2.0) * hit.depth / 48.0
                }
                Projection::Orthographic { view_height } => view_height / 48.0,
            };
            let slack = 1e-3 + scale * 1e-14 / pixel_mm;
            let within = match hit.element {
                Element::Face(_) => slack,
                Element::Edge(_) => HIT_EDGE_POINTS * ppp + slack,
                Element::Vertex(_) => HIT_VERTEX_POINTS * ppp + slack,
            };
            if let Some(at) = project(&camera, size_px, hit.point) {
                proptest::prop_assert!(off(at) <= within, "{:?} lands {} off", hit, off(at));
            }
        }
        let listed = |element: Element| under.iter().any(|hit| hit.element == element);
        // The reverse: what lands near enough is listed.
        for (number, vertex) in (1u32..).zip(&mesh.vertex_points) {
            if let Some(at) = project(&camera, size_px, *vertex)
                && off(at) < HIT_VERTEX_POINTS * ppp - 1e-6
            {
                proptest::prop_assert!(listed(Element::Vertex(number)), "Vertex{} lands {} off: {:?}", number, off(at), under);
            }
        }
        for (number, ends) in (1u32..).zip(mesh.edge_points.chunks(2)) {
            let (Some(a), Some(b)) = (project(&camera, size_px, ends[0]), project(&camera, size_px, ends[1])) else {
                continue;
            };
            let d = [b[0] - a[0], b[1] - a[1]];
            let length = d[0] * d[0] + d[1] * d[1];
            let s = if length == 0.0 {
                0.0
            } else {
                (((pointer[0] - a[0]) * d[0] + (pointer[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0)
            };
            let nearest = off([a[0] + d[0] * s, a[1] + d[1] * s]);
            if nearest < HIT_EDGE_POINTS * ppp - 1e-6 {
                proptest::prop_assert!(listed(Element::Edge(number)), "Edge{} is {} off: {:?}", number, nearest, under);
            }
        }
        // The faces, by where the pointer's line crosses the box's slabs.
        let Some((origin, direction)) = ray(&camera, size_px, pointer) else {
            return Ok(());
        };
        let (mut box_low, mut box_high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in &mesh.positions {
            for k in 0..3 {
                box_low[k] = box_low[k].min(p[k]);
                box_high[k] = box_high[k].max(p[k]);
            }
        }
        let scale = low.iter().chain(&eye).map(|c| c.abs()).fold(1.0, f64::max);
        let tolerance = 1e-5 * diagonal + 1e-12 * scale;
        let mut enters: Vec<(f64, u32)> = Vec::new();
        let mut leaves: Vec<(f64, u32)> = Vec::new();
        let mut misses = false;
        let mut unclear = false;
        for k in 0..3 {
            let (lo, hi, o, d) = (box_low[k], box_high[k], origin[k], direction[k]);
            let low_face = 2 * u32::try_from(k).unwrap() + 1;
            if d.abs() < 1e-12 {
                // Along the slab: inside it or not, clearly.
                if o < lo - tolerance || o > hi + tolerance {
                    misses = true;
                } else if o < lo + tolerance || o > hi - tolerance {
                    unclear = true;
                }
                continue;
            }
            let (t_low, t_high) = ((lo - o) / d, (hi - o) / d);
            if d > 0.0 {
                enters.push((t_low, low_face));
                leaves.push((t_high, low_face + 1));
            } else {
                enters.push((t_high, low_face + 1));
                leaves.push((t_low, low_face));
            }
        }
        enters.sort_by(|a, b| b.0.total_cmp(&a.0));
        leaves.sort_by(|a, b| a.0.total_cmp(&b.0));
        let faces: Vec<(Element, bool)> = under
            .iter()
            .filter(|hit| matches!(hit.element, Element::Face(_)))
            .map(|hit| (hit.element, hit.hidden))
            .collect();
        let close = |pair: &[(f64, u32)]| pair.len() > 1 && (pair[0].0 - pair[1].0).abs() <= tolerance;
        if unclear || close(&enters) || close(&leaves) {
            return Ok(());
        }
        let (Some(&(enter, entry)), Some(&(leave, exit))) = (enters.first(), leaves.first()) else {
            return Ok(());
        };
        if misses || leave < enter - tolerance {
            proptest::prop_assert!(faces.is_empty(), "the line misses the box: {:?}", under);
        } else if leave > enter + tolerance {
            proptest::prop_assert_eq!(
                &faces,
                &vec![(Element::Face(entry), false), (Element::Face(exit), true)],
                "entering through Face{} at {}, leaving through Face{} at {}: {:?}",
                entry, enter, exit, leave, under
            );
        }
    }
}

/// An offset of the box: none, small, large, or as far as a billion millimetres.
fn offset() -> impl proptest::strategy::Strategy<Value = f64> {
    use proptest::prelude::*;
    prop_oneof![Just(0.0), -1e3..1e3f64, -1e6..1e6f64, Just(1e9), Just(-1e9)]
}

/// A side of a box resolved at any [`offset`]: a micron to a hundred metres.
fn resolved_side() -> impl proptest::strategy::Strategy<Value = f64> {
    use proptest::prelude::*;
    prop_oneof![Just(1e-3), 1e-2..1e3f64, Just(1e5)]
}
