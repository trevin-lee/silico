//! [`hits`], Select Other's cast (M5.7), worked by hand: what lies under a pointer, nearest
//! the eye first, each element numbered as FreeCAD numbers a Part box (and as the kernel's
//! tests pin it: `silico-kernel/tests/mesh.rs`, `topology.rs`, `adversarial_distance.rs`).

use std::sync::Arc;

use silico_protocol::{Camera, MAX_RENDER_COORDINATE, Mesh, Projection};

use super::{
    Element, HIT_EDGE_POINTS, HIT_VERTEX_POINTS, Hit, MAX_HITS, MIN_EYE_DISTANCE, Pickable,
    VERTEX_REACH, hits, project,
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
    // (20, 0) and (20, 20), are 50 pixels off). The ray meets the top, Face6, at depth 80,
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

    /// Whatever the box, camera, and pointer: no panic, the same list each time, each hit
    /// one of the box's elements, at a finite point, once; each face's point on the pointer's
    /// pixel and each edge's and vertex's within its threshold of it, by the renderer's own
    /// projection (`project`); the faces nearest first, the nearest of them alone unhidden.
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
}
