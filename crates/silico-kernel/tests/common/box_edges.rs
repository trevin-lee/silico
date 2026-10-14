//! The hand calculation behind the tests of which edges a failed fillet or chamfer names,
//! on an axis-aligned box: a chamfer of an edge reaches `distance` across its reference
//! face (the lower-numbered of its two, or the higher when flipped) and `other` (or
//! `distance` tan(angle)) across the other face, and a fillet of radius r reaches r across
//! each, as a chamfer of equal distances r does. It runs past a face when a reach is over
//! that face's width, and two that share no corner collide when their blocks overlap. Box
//! faces are numbered as FreeCAD 1.1.3 numbers `Part.makeBox(a, b, c).Faces` (x = 0,
//! x = a, y = 0, y = b, z = 0, z = c), so an edge's lower-numbered face is the one across
//! the lower of its two cross axes; edges as `Part.makeBox(a, b, c).Edges` (`EDGES`).

use silico_kernel::{Built, ChamferSize, KernelError, Refusal};

/// Each box edge: the axis it runs along, and its place across the other two axes (in
/// increasing axis order), 0 at the low face and 1 at the high. From FreeCAD 1.1.3:
/// `Part.makeBox(10, 20, 30).Edges[0]` runs from (0, 0, 30) to (0, 0, 0), and so on.
pub const EDGES: [(usize, [u8; 2]); 12] = [
    (2, [0, 0]), // Edge1
    (1, [0, 1]), // Edge2
    (2, [0, 1]), // Edge3
    (1, [0, 0]), // Edge4
    (2, [1, 0]), // Edge5
    (1, [1, 1]), // Edge6
    (2, [1, 1]), // Edge7
    (1, [1, 0]), // Edge8
    (0, [0, 0]), // Edge9
    (0, [0, 1]), // Edge10
    (0, [1, 0]), // Edge11
    (0, [1, 1]), // Edge12
];

/// tan of the angles a distance-and-angle case draws from, in square roots.
pub fn exact_tan(degrees: f64) -> f64 {
    let (r2, r3) = (2f64.sqrt(), 3f64.sqrt());
    match degrees {
        15.0 => 2.0 - r3,
        22.5 => r2 - 1.0,
        30.0 => 1.0 / r3,
        45.0 => 1.0,
        60.0 => r3,
        67.5 => r2 + 1.0,
        75.0 => 2.0 + r3,
        _ => panic!("no exact tan for {degrees}"),
    }
}

/// How far a chamfer of `size` on box edge `e` reaches along each of the edge's two cross
/// axes (p, q, p < q): the reference face (across p's face, or q's when flipped) takes
/// `distance` along the other cross axis.
pub fn reach(e: u32, size: ChamferSize, flip: bool) -> [(usize, f64); 2] {
    let along = EDGES[(e - 1) as usize].0;
    let cross: Vec<usize> = (0..3).filter(|&a| a != along).collect();
    let (p, q) = (cross[0], cross[1]);
    let (on_reference, on_other) = match size {
        ChamferSize::Equal { distance } => (distance, distance),
        ChamferSize::TwoDistances { distance, other } => (distance, other),
        ChamferSize::DistanceAngle { distance, angle } => (distance, distance * exact_tan(angle)),
    };
    // The reference face is the one across p (unflipped): it spans q, so `distance` is a
    // reach along q, and the other face's reach is along p.
    if flip {
        [(p, on_reference), (q, on_other)]
    } else {
        [(q, on_reference), (p, on_other)]
    }
}

/// The block a chamfer on box edge `e` cuts into: an interval on each axis.
pub fn strip(dims: [f64; 3], e: u32, size: ChamferSize, flip: bool) -> [(f64, f64); 3] {
    let (along, sides) = EDGES[(e - 1) as usize];
    let cross: Vec<usize> = (0..3).filter(|&a| a != along).collect();
    let mut out = [(0.0, 0.0); 3];
    out[along] = (0.0, dims[along]);
    for (axis, r) in reach(e, size, flip) {
        let side = sides[cross.iter().position(|&a| a == axis).unwrap()];
        out[axis] = if side == 0 {
            (0.0, r)
        } else {
            (dims[axis] - r, dims[axis])
        };
    }
    out
}

/// The box corners edge `e` ends at.
pub fn ends(e: u32) -> [[u8; 3]; 2] {
    let (along, sides) = EDGES[(e - 1) as usize];
    let mut sides = sides.iter();
    let mut low = [0u8; 3];
    for (axis, slot) in low.iter_mut().enumerate() {
        if axis != along {
            *slot = *sides.next().unwrap();
        }
    }
    let mut high = low;
    high[along] = 1;
    [low, high]
}

/// Whether the chamfer of `e` alone runs past one of its faces (by more than `m`); None
/// within `m` of it.
pub fn runs_past(dims: [f64; 3], e: u32, size: ChamferSize, flip: bool, m: f64) -> Option<bool> {
    let mut near = false;
    for (axis, r) in reach(e, size, flip) {
        if r > dims[axis] + m {
            return Some(true);
        }
        near |= (r - dims[axis]).abs() <= m;
    }
    if near { None } else { Some(false) }
}

/// Whether `e` is at fault among `given`: it runs past a face, or its block overlaps (by
/// more than `m`) that of another given edge it shares no corner with. None near either.
pub fn at_fault(
    dims: [f64; 3],
    given: &[u32],
    e: u32,
    size: ChamferSize,
    flip: bool,
    m: f64,
) -> Option<bool> {
    let mut near = match runs_past(dims, e, size, flip, m) {
        Some(true) => return Some(true),
        Some(false) => false,
        None => true,
    };
    let mine = strip(dims, e, size, flip);
    for &other in given {
        if other == e || ends(other).iter().any(|c| ends(e).contains(c)) {
            continue;
        }
        let theirs = strip(dims, other, size, flip);
        let depth = (0..3)
            .map(|a| mine[a].1.min(theirs[a].1) - mine[a].0.max(theirs[a].0))
            .fold(f64::INFINITY, f64::min);
        if depth > m {
            return Some(true);
        }
        near |= depth.abs() <= m;
    }
    if near { None } else { Some(false) }
}

/// The refusal of an operation expected to fail.
pub fn failed(result: Result<Built, KernelError>) -> KernelError {
    match result {
        Ok(_) => panic!("built"),
        Err(e) => e,
    }
}

/// The edges a failed fillet or chamfer names, if it names some.
pub fn named(err: &KernelError) -> Option<Vec<u32>> {
    match err.refusal() {
        Refusal::EdgesFailed { edges, .. } => Some(edges.clone()),
        _ => None,
    }
}
