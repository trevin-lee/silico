//! The scene the render bench draws and the render tests read by hand: a 40 x 20 x 10 box
//! from the origin, meshed as OCCT orders a box's faces, seen from the front off its centre
//! (docs/design/m4.6-render.md, Tests).

use silico_protocol::{Camera, Mesh, Projection, RenderBody, RenderScene};

/// A box from `low` of size `size`, meshed as OCCT orders a box's faces (x low, x high,
/// y low, y high, z low, z high), two triangles each, counterclockwise from outside, each
/// face's vertices its own with its normal. Its edges are 2-point polylines, the front
/// face's (y low) four first: bottom, right, top, left; then the back face's, then the four
/// along y.
pub fn box_mesh(low: [f64; 3], size: [f64; 3]) -> Mesh {
    let [x0, y0, z0] = low;
    let [x1, y1, z1] = [low[0] + size[0], low[1] + size[1], low[2] + size[2]];
    let faces: [([f32; 3], [[f64; 3]; 4]); 6] = [
        (
            [-1., 0., 0.],
            [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        ),
        (
            [1., 0., 0.],
            [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
        ),
        (
            [0., -1., 0.],
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        ),
        (
            [0., 1., 0.],
            [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        ),
        (
            [0., 0., -1.],
            [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
        ),
        (
            [0., 0., 1.],
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        ),
    ];
    let mut mesh = Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles: vec![],
        edge_points: vec![],
        edge_point_counts: vec![],
        valid: None,
    };
    for (base, (normal, corners)) in (0u32..).step_by(4).zip(faces) {
        mesh.positions.extend(corners);
        mesh.normals.extend([normal; 4]);
        mesh.triangles
            .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        mesh.face_triangles.push(2);
    }
    let edges = [
        ([x0, y0, z0], [x1, y0, z0]),
        ([x1, y0, z0], [x1, y0, z1]),
        ([x1, y0, z1], [x0, y0, z1]),
        ([x0, y0, z1], [x0, y0, z0]),
        ([x0, y1, z0], [x1, y1, z0]),
        ([x1, y1, z0], [x1, y1, z1]),
        ([x1, y1, z1], [x0, y1, z1]),
        ([x0, y1, z1], [x0, y1, z0]),
        ([x0, y0, z0], [x0, y1, z0]),
        ([x1, y0, z0], [x1, y1, z0]),
        ([x1, y0, z1], [x1, y1, z1]),
        ([x0, y0, z1], [x0, y1, z1]),
    ];
    for (a, b) in edges {
        mesh.edge_points.extend([a, b]);
        mesh.edge_point_counts.push(2);
    }
    mesh
}

/// The view's height in [`front_of_a_box`].
const VIEW_HEIGHT: f64 = 44.0;

/// The 40 x 20 x 10 box from the origin, body 1, from the front (from −y, z up),
/// orthographic, 44 mm high about (21, 10, 7), `width` by `height` pixels, with
/// its ID buffer. Its front face lands at x −21 to 19 mm and z −7 to 3 mm about the view's
/// centre: off the centre both ways, so a flipped or mirrored image shows, and every edge
/// in view at any aspect from square to wider.
pub fn front_of_a_box(width: u32, height: u32) -> RenderScene {
    RenderScene {
        bodies: vec![RenderBody {
            body: 1,
            mesh: box_mesh([0.0; 3], [40.0, 20.0, 10.0]),
            highlight_faces: vec![],
            highlight_edges: vec![],
        }],
        camera: Camera {
            projection: Projection::Orthographic {
                view_height: VIEW_HEIGHT,
            },
            eye: [21.0, -50.0, 7.0],
            target: [21.0, 10.0, 7.0],
            up: [0.0, 0.0, 1.0],
        },
        width,
        height,
        ids: true,
    }
}
