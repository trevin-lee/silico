//! Sketches in PlaneGCS's terms, built by hand for the tests (included with `#[path]`).
#![allow(dead_code)]

use silico_protocol::{
    Algorithm, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem, SolverConstraint, Tag,
    Value,
};

/// Builds a system value by value, in order.
#[derive(Default)]
pub struct Builder {
    pub values: Vec<f64>,
    pub unknowns: Vec<Value>,
    pub geometry: Vec<SketchGeometry>,
    pub constraints: Vec<SolverConstraint>,
}

impl Builder {
    /// A value the solver may change, starting at `v`.
    pub fn unknown(&mut self, v: f64) -> Value {
        let at = self.fixed(v);
        self.unknowns.push(at);
        at
    }

    /// A value the solver keeps.
    pub fn fixed(&mut self, v: f64) -> Value {
        self.values.push(v);
        Value((self.values.len() - 1) as u32)
    }

    pub fn point_values(&mut self, x: f64, y: f64) -> PointValues {
        PointValues {
            x: self.unknown(x),
            y: self.unknown(y),
        }
    }

    pub fn add(&mut self, g: SketchGeometry) -> Geometry {
        self.geometry.push(g);
        Geometry((self.geometry.len() - 1) as u32)
    }

    pub fn line(&mut self, a: (f64, f64), b: (f64, f64)) -> Geometry {
        let start = self.point_values(a.0, a.1);
        let end = self.point_values(b.0, b.1);
        self.add(SketchGeometry::Line { start, end })
    }

    /// A point the solver keeps where it is.
    pub fn fixed_point(&mut self, x: f64, y: f64) -> Geometry {
        let p = PointValues {
            x: self.fixed(x),
            y: self.fixed(y),
        };
        self.add(SketchGeometry::Point(p))
    }

    /// The next tag: constraints are numbered from 1 in the order they're added.
    pub fn tag(&self) -> Tag {
        Tag(self
            .constraints
            .iter()
            .filter(|c| c.tag().is_some())
            .count() as u32
            + 1)
    }

    pub fn constrain(&mut self, c: impl FnOnce(Tag) -> SolverConstraint) -> Tag {
        let tag = self.tag();
        self.constraints.push(c(tag));
        tag
    }

    pub fn system(self, algorithm: Algorithm) -> SketchSystem {
        self.system_trying(vec![algorithm])
    }

    /// The system, its algorithms tried in this order.
    pub fn system_trying(self, algorithms: Vec<Algorithm>) -> SketchSystem {
        SketchSystem {
            values: self.values,
            unknowns: self.unknowns,
            geometry: self.geometry,
            constraints: self.constraints,
            algorithms,
        }
    }
}

/// The lines of a rectangle, and the system's builder, its four sides drawn roughly:
/// bottom, right, top, left, each from the corner the one before ends at.
pub struct Rectangle {
    pub b: Builder,
    pub sides: [Geometry; 4],
}

/// A rectangle as FreeCAD's `CreateRectangleSketch` constrains one, without its two
/// lengths: its sides joined end to start, the bottom and top horizontal, the others
/// vertical, its bottom-left corner pinned to a fixed point at (0, 0). Its corners start
/// off their places, so the solver has work to do.
pub fn rectangle() -> Rectangle {
    let mut b = Builder::default();
    let origin = b.fixed_point(0.0, 0.0);
    let bottom = b.line((0.3, -0.2), (37.0, 1.5));
    let right = b.line((38.5, 0.7), (41.0, 23.0));
    let top = b.line((40.2, 19.1), (-1.0, 21.7));
    let left = b.line((0.9, 18.4), (-0.6, 0.4));
    let sides = [bottom, right, top, left];
    for i in 0..4 {
        let (a, next) = (sides[i], sides[(i + 1) % 4]);
        b.constrain(|tag| SolverConstraint::Coincident {
            tag,
            a: PointOf::End(a),
            b: PointOf::Start(next),
        });
    }
    b.constrain(|tag| SolverConstraint::Horizontal { tag, line: bottom });
    b.constrain(|tag| SolverConstraint::Horizontal { tag, line: top });
    b.constrain(|tag| SolverConstraint::Vertical { tag, line: right });
    b.constrain(|tag| SolverConstraint::Vertical { tag, line: left });
    b.constrain(|tag| SolverConstraint::Coincident {
        tag,
        a: PointOf::Start(bottom),
        b: PointOf::Point(origin),
    });
    Rectangle { b, sides }
}

/// [`rectangle`], 40 wide and 20 high: every degree of freedom taken.
pub fn rectangle_40_by_20() -> Rectangle {
    let mut r = rectangle();
    let [bottom, right, ..] = r.sides;
    let width = r.b.fixed(40.0);
    let height = r.b.fixed(20.0);
    r.b.constrain(|tag| SolverConstraint::DistanceX {
        tag,
        a: PointOf::Start(bottom),
        b: PointOf::End(bottom),
        value: width,
    });
    r.b.constrain(|tag| SolverConstraint::DistanceY {
        tag,
        a: PointOf::Start(right),
        b: PointOf::End(right),
        value: height,
    });
    r
}

/// A line's start and end from a solution, as `[sx, sy, ex, ey]`.
pub fn line_values(system: &SketchSystem, values: &[f64], line: Geometry) -> [f64; 4] {
    match system.geometry[line.0 as usize] {
        SketchGeometry::Line { start, end } => {
            [start.x, start.y, end.x, end.y].map(|v| values[v.0 as usize])
        }
        other => panic!("geometry {} is {other:?}", line.0),
    }
}

/// SplitMix64: a fixed sequence from a seed, the same on every platform.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// In [lo, hi), from the top 53 bits: exact arithmetic, no platform math.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * unit
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Whether a process with this ID exists (zombies included).
pub fn process_exists(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap_or_else(|e| panic!("ps: {e}"))
        .status
        .success()
}
