//! For the sketch worker's property tests: generated sketches, and each constraint's
//! residual written out by hand as PlaneGCS defines it (an oracle independent of the
//! bridge). Shared by adversarial_planegcs.rs and adversarial_drag.rs.

use silico_protocol::{
    Algorithm, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem, SolverConstraint,
    Value,
};

use crate::systems::{Builder, Rng};

/// A generator of sketches biased to the degenerate: coincident points, zero lengths and
/// radii, zero and negative dimensions, constraints on fixed values, and (when `stray`) now
/// and then a reference the check must refuse.
pub struct Gen {
    pub rng: Rng,
    pub b: Builder,
    pub coords: Vec<f64>,
    /// Whether a reference may (rarely) name any geometry, or one past the end.
    pub stray: bool,
}

impl Gen {
    fn coord(&mut self) -> f64 {
        let v = match self.rng.below(10) {
            0 | 1 if !self.coords.is_empty() => {
                self.coords[self.rng.below(self.coords.len() as u64) as usize]
            }
            2 => self.rng.below(5) as f64 - 2.0,
            _ => self.rng.range(-50.0, 50.0),
        };
        self.coords.push(v);
        v
    }
    fn value(&mut self, v: f64) -> Value {
        if self.rng.below(5) == 0 {
            self.b.fixed(v)
        } else {
            self.b.unknown(v)
        }
    }
    fn point(&mut self) -> PointValues {
        let (x, y) = (self.coord(), self.coord());
        PointValues {
            x: self.value(x),
            y: self.value(y),
        }
    }
    fn length(&mut self) -> f64 {
        match self.rng.below(12) {
            0 => 0.0,
            1 => -self.rng.range(0.0, 5.0),
            _ => self.rng.range(0.0, 60.0),
        }
    }
    fn geometry(&mut self) {
        match self.rng.below(4) {
            0 => {
                let p = self.point();
                self.b.add(SketchGeometry::Point(p));
            }
            1 => {
                let (start, end) = (self.point(), self.point());
                self.b.add(SketchGeometry::Line { start, end });
            }
            2 => {
                let center = self.point();
                let r = self.length().abs();
                let radius = self.value(r);
                self.b.add(SketchGeometry::Circle { center, radius });
            }
            _ => {
                let center = self.point();
                let r = self.length().abs();
                let radius = self.value(r);
                let sa = self.rng.range(-1.0, 7.0);
                let ea = self.rng.range(-1.0, 7.0);
                let start_angle = self.value(sa);
                let end_angle = self.value(ea);
                let (start, end) = (self.point(), self.point());
                let arc = self.b.add(SketchGeometry::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                    start,
                    end,
                });
                if self.rng.below(10) > 0 {
                    self.b.constraints.push(SolverConstraint::ArcRules { arc });
                }
            }
        }
    }
    fn kind(&self, g: Geometry) -> u8 {
        match self.b.geometry[g.0 as usize] {
            SketchGeometry::Point(_) => 0,
            SketchGeometry::Line { .. } => 1,
            SketchGeometry::Circle { .. } => 2,
            SketchGeometry::Arc { .. } => 3,
        }
    }
    /// Some geometry of one of `kinds`, or (rarely, when `stray`) any geometry or one past
    /// the end.
    fn pick(&mut self, kinds: &[u8]) -> Option<Geometry> {
        let n = self.b.geometry.len() as u32;
        if self.stray && self.rng.below(40) == 0 {
            return Some(Geometry(self.rng.below(n as u64 + 1) as u32));
        }
        let fits: Vec<Geometry> = (0..n)
            .map(Geometry)
            .filter(|&g| kinds.contains(&self.kind(g)))
            .collect();
        if fits.is_empty() {
            None
        } else {
            Some(fits[self.rng.below(fits.len() as u64) as usize])
        }
    }
    fn point_of(&mut self) -> Option<PointOf> {
        let g = self.pick(&[0, 1, 2, 3])?;
        if g.0 as usize >= self.b.geometry.len() {
            return Some(PointOf::Start(g));
        }
        let choices = points_of(self.b.geometry[g.0 as usize], g);
        Some(choices[self.rng.below(choices.len() as u64) as usize])
    }
    fn constraint(&mut self) {
        let tag = self.b.tag();
        let c = match self.rng.below(12) {
            0 => {
                let (a, b) = (self.point_of(), self.point_of());
                a.zip(b)
                    .map(|(a, b)| SolverConstraint::Coincident { tag, a, b })
            }
            1 => self
                .pick(&[1])
                .map(|line| SolverConstraint::Horizontal { tag, line }),
            2 => self
                .pick(&[1])
                .map(|line| SolverConstraint::Vertical { tag, line }),
            k @ 3..=5 => {
                let (a, b) = (self.point_of(), self.point_of());
                let d = self.length();
                let value = self.b.fixed(d);
                a.zip(b).map(|(a, b)| match k {
                    3 => SolverConstraint::DistanceX { tag, a, b, value },
                    4 => SolverConstraint::DistanceY { tag, a, b, value },
                    _ => SolverConstraint::Distance { tag, a, b, value },
                })
            }
            6 => {
                let (p, l) = (self.point_of(), self.pick(&[1]));
                p.zip(l)
                    .map(|(point, line)| SolverConstraint::PointOnLine { tag, point, line })
            }
            k @ 7..=10 => {
                let (a, b) = (self.pick(&[1]), self.pick(&[1]));
                let angle = self.rng.range(-3.5, 3.5);
                let value = self.b.fixed(angle);
                a.zip(b).map(|(a, b)| match k {
                    7 => SolverConstraint::Parallel { tag, a, b },
                    8 => SolverConstraint::Perpendicular { tag, a, b },
                    9 => SolverConstraint::EqualLength { tag, a, b },
                    _ => SolverConstraint::Angle {
                        tag,
                        a,
                        b,
                        reversed: false,
                        value,
                    },
                })
            }
            _ => {
                let c = self.pick(&[2, 3]);
                let r = self.length();
                let value = self.b.fixed(r);
                c.map(|circle| SolverConstraint::Radius { tag, circle, value })
            }
        };
        if let Some(c) = c {
            self.b.constraints.push(c);
        }
    }
}

/// The sketch `seed` makes: references stray when `stray`, and solved with `algorithm`, or
/// one of the three at random.
pub fn generate(seed: u64, stray: bool, algorithm: Option<Algorithm>) -> SketchSystem {
    let mut g = Gen {
        rng: Rng(seed),
        b: Builder::default(),
        coords: Vec::new(),
        stray,
    };
    for _ in 0..1 + g.rng.below(5) {
        g.geometry();
    }
    for _ in 0..g.rng.below(9) {
        g.constraint();
    }
    let algorithm = algorithm.unwrap_or_else(|| match g.rng.below(3) {
        0 => Algorithm::DogLeg,
        1 => Algorithm::LevenbergMarquardt,
        _ => Algorithm::Bfgs,
    });
    g.b.system(algorithm)
}

/// Every point a piece of geometry has, as a drag or a constraint names it.
pub fn points_of(g: SketchGeometry, at: Geometry) -> Vec<PointOf> {
    match g {
        SketchGeometry::Point(_) => vec![PointOf::Point(at)],
        SketchGeometry::Line { .. } => vec![PointOf::Start(at), PointOf::End(at)],
        SketchGeometry::Circle { .. } => vec![PointOf::Center(at)],
        SketchGeometry::Arc { .. } => {
            vec![PointOf::Start(at), PointOf::End(at), PointOf::Center(at)]
        }
    }
}

/// Where point `p` is at the values `v`.
pub fn xy(s: &SketchSystem, v: &[f64], p: PointOf) -> (f64, f64) {
    let at = |pv: PointValues| (v[pv.x.0 as usize], v[pv.y.0 as usize]);
    let (g, which) = match p {
        PointOf::Point(g) => (g, 0),
        PointOf::Start(g) => (g, 1),
        PointOf::End(g) => (g, 2),
        PointOf::Center(g) => (g, 3),
    };
    match (s.geometry[g.0 as usize], which) {
        (SketchGeometry::Point(p), 0) => at(p),
        (SketchGeometry::Line { start, .. }, 1) | (SketchGeometry::Arc { start, .. }, 1) => {
            at(start)
        }
        (SketchGeometry::Line { end, .. }, 2) | (SketchGeometry::Arc { end, .. }, 2) => at(end),
        (SketchGeometry::Circle { center, .. }, 3) | (SketchGeometry::Arc { center, .. }, 3) => {
            at(center)
        }
        other => panic!("{other:?}"),
    }
}

/// A line's direction, end less start.
pub fn dir(s: &SketchSystem, v: &[f64], l: Geometry) -> (f64, f64) {
    let (a, b) = (xy(s, v, PointOf::Start(l)), xy(s, v, PointOf::End(l)));
    (b.0 - a.0, b.1 - a.1)
}

/// A circle's or arc's radius at `v`.
pub fn radius(s: &SketchSystem, v: &[f64], g: Geometry) -> f64 {
    match s.geometry[g.0 as usize] {
        SketchGeometry::Circle { radius, .. } | SketchGeometry::Arc { radius, .. } => {
            v[radius.0 as usize]
        }
        other => panic!("{other:?}"),
    }
}

/// Each equation's residual at `v`, as PlaneGCS defines it (its scales are 1 but for
/// parallel and perpendicular, 1/(|l1||l2|) at the values given), written out here.
pub fn residuals(s: &SketchSystem, v: &[f64], c: &SolverConstraint) -> Vec<f64> {
    let len = |d: (f64, f64)| libm::hypot(d.0, d.1);
    match *c {
        SolverConstraint::ArcRules { arc } => {
            let SketchGeometry::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                start,
                end,
            } = s.geometry[arc.0 as usize]
            else {
                panic!("arc rules on {arc:?}, which isn't an arc")
            };
            let (cx, cy, r) = (
                v[center.x.0 as usize],
                v[center.y.0 as usize],
                v[radius.0 as usize],
            );
            let on = |p: PointValues, a: Value| {
                let a = v[a.0 as usize];
                vec![
                    v[p.x.0 as usize] - (cx + r * libm::cos(a)),
                    v[p.y.0 as usize] - (cy + r * libm::sin(a)),
                ]
            };
            [on(start, start_angle), on(end, end_angle)].concat()
        }
        SolverConstraint::Coincident { a, b, .. } => {
            let (a, b) = (xy(s, v, a), xy(s, v, b));
            vec![a.0 - b.0, a.1 - b.1]
        }
        SolverConstraint::Horizontal { line, .. } => vec![dir(s, v, line).1],
        SolverConstraint::Vertical { line, .. } => vec![dir(s, v, line).0],
        SolverConstraint::DistanceX { a, b, value, .. } => {
            vec![xy(s, v, b).0 - xy(s, v, a).0 - v[value.0 as usize]]
        }
        SolverConstraint::DistanceY { a, b, value, .. } => {
            vec![xy(s, v, b).1 - xy(s, v, a).1 - v[value.0 as usize]]
        }
        SolverConstraint::Distance { a, b, value, .. } => {
            let (a, b) = (xy(s, v, a), xy(s, v, b));
            vec![libm::hypot(a.0 - b.0, a.1 - b.1) - v[value.0 as usize]]
        }
        SolverConstraint::PointOnLine { point, line, .. } => {
            let (x0, y0) = xy(s, v, point);
            let (x1, y1) = xy(s, v, PointOf::Start(line));
            let (x2, y2) = xy(s, v, PointOf::End(line));
            let (dx, dy) = (x2 - x1, y2 - y1);
            vec![(-x0 * dy + y0 * dx + x1 * y2 - x2 * y1) / libm::hypot(dx, dy)]
        }
        SolverConstraint::Parallel { a, b, .. } | SolverConstraint::Perpendicular { a, b, .. } => {
            let (d1, d2) = (dir(s, v, a), dir(s, v, b));
            let scale = len(dir(s, &s.values, a)) * len(dir(s, &s.values, b));
            let e = if matches!(c, SolverConstraint::Parallel { .. }) {
                d1.0 * d2.1 - d1.1 * d2.0
            } else {
                d1.0 * d2.0 + d1.1 * d2.1
            };
            vec![e / scale]
        }
        SolverConstraint::Angle {
            a,
            b,
            reversed,
            value,
            ..
        } => {
            // Reversed, b's direction runs from its end to its start (PlaneGCS's l2p1 and
            // l2p2 swapped).
            let (d1, d2) = (dir(s, v, a), dir(s, v, b));
            let d2 = if reversed { (-d2.0, -d2.1) } else { d2 };
            let t = libm::atan2(d1.1, d1.0) + v[value.0 as usize];
            let (x2, y2) = (
                d2.0 * libm::cos(t) + d2.1 * libm::sin(t),
                -d2.0 * libm::sin(t) + d2.1 * libm::cos(t),
            );
            vec![libm::atan2(y2, x2)]
        }
        SolverConstraint::EqualLength { a, b, .. } => {
            vec![len(dir(s, v, b)) - len(dir(s, v, a))]
        }
        SolverConstraint::Radius { circle, value, .. } => {
            vec![radius(s, v, circle) - v[value.0 as usize]]
        }
        SolverConstraint::HorizontalPoints { a, b, .. } => vec![xy(s, v, a).1 - xy(s, v, b).1],
        SolverConstraint::VerticalPoints { a, b, .. } => vec![xy(s, v, a).0 - xy(s, v, b).0],
        SolverConstraint::PointLineDistance {
            point, line, value, ..
        } => vec![from_line(s, v, xy(s, v, point), line).abs() - v[value.0 as usize]],
        SolverConstraint::PointOnCircle { point, circle, .. } => {
            vec![off_circle(s, v, xy(s, v, point), circle)]
        }
        SolverConstraint::EqualRadius { a, b, .. } => {
            // As the bridge sends it: a circle's radius before an arc's.
            let arc = |g: Geometry| matches!(s.geometry[g.0 as usize], SketchGeometry::Arc { .. });
            let (a, b) = if arc(a) && !arc(b) { (b, a) } else { (a, b) };
            vec![radius(s, v, a) - radius(s, v, b)]
        }
        SolverConstraint::TangentLine { line, circle, .. } => vec![
            from_line(s, v, xy(s, v, PointOf::Center(circle)), line).abs() - radius(s, v, circle),
        ],
        SolverConstraint::TangentCircles { a, b, internal, .. } => {
            let (ca, cb) = (xy(s, v, PointOf::Center(a)), xy(s, v, PointOf::Center(b)));
            let (dx, dy) = (ca.0 - cb.0, ca.1 - cb.1);
            let d_sq = dx * dx + dy * dy;
            let (ra, rb) = (radius(s, v, a), radius(s, v, b));
            let r = if internal { ra - rb } else { ra + rb };
            vec![if d_sq < 1e-14 { ra - rb } else { d_sq - r * r }]
        }
        SolverConstraint::TangentOn {
            a, point, b, angle, ..
        } => {
            let p = xy(s, v, point);
            let on = match s.geometry[b.0 as usize] {
                SketchGeometry::Line { .. } => from_line(s, v, p, b),
                _ => off_circle(s, v, p, b),
            };
            vec![on, angle_via_point(s, v, a, b, p, v[angle.0 as usize])]
        }
        SolverConstraint::AngleViaPoint {
            a, b, point, angle, ..
        } => vec![angle_via_point(
            s,
            v,
            a,
            b,
            xy(s, v, point),
            v[angle.0 as usize],
        )],
        SolverConstraint::SymmetricLine { a, b, line, .. } => {
            let (pa, pb) = (xy(s, v, a), xy(s, v, b));
            let ab = (pa.0 - pb.0, pa.1 - pb.1);
            let l = dir(s, v, line);
            let (sa, sb) = (xy(s, &s.values, a), xy(s, &s.values, b));
            let scale = len((sa.0 - sb.0, sa.1 - sb.1)) * len(dir(s, &s.values, line));
            let mid = ((pa.0 + pb.0) / 2.0, (pa.1 + pb.1) / 2.0);
            vec![
                (ab.0 * -l.0 + ab.1 * -l.1) / scale,
                from_line(s, v, mid, line),
            ]
        }
        SolverConstraint::MidpointOnLine { a, b, line, .. } => {
            let (pa, pb) = (xy(s, v, a), xy(s, v, b));
            vec![from_line(
                s,
                v,
                ((pa.0 + pb.0) / 2.0, (pa.1 + pb.1) / 2.0),
                line,
            )]
        }
        SolverConstraint::SymmetricPoint { a, b, center, .. } => {
            let (pa, pb, m) = (xy(s, v, a), xy(s, v, b), xy(s, v, center));
            let d = len((pb.0 - pa.0, pb.1 - pa.1));
            let u = ((pb.0 - pa.0) / d, (pb.1 - pa.1) / d);
            let along = |q: (f64, f64)| (m.0 - q.0) * u.0 + (m.1 - q.1) * u.1;
            let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
            let off = (-m.0 * dy + m.1 * dx + pa.0 * pb.1 - pb.0 * pa.1) / d;
            vec![along(pa) + along(pb), off]
        }
    }
}

/// The signed distance of `p` from line `l` (extended), left of its direction positive, as
/// PlaneGCS's `PointOnLine` measures it.
pub fn from_line(s: &SketchSystem, v: &[f64], p: (f64, f64), l: Geometry) -> f64 {
    let (x1, y1) = xy(s, v, PointOf::Start(l));
    let (x2, y2) = xy(s, v, PointOf::End(l));
    let (dx, dy) = (x2 - x1, y2 - y1);
    (-p.0 * dy + p.1 * dx + x1 * y2 - x2 * y1) / (dx * dx + dy * dy).sqrt()
}

/// How far `p` is from circle (or arc) `c`'s circle, outward positive.
pub fn off_circle(s: &SketchSystem, v: &[f64], p: (f64, f64), c: Geometry) -> f64 {
    let o = xy(s, v, PointOf::Center(c));
    let (dx, dy) = (p.0 - o.0, p.1 - o.1);
    (dx * dx + dy * dy).sqrt() - radius(s, v, c)
}

/// PlaneGCS's angle at a point: from `a`'s normal at `p`, turned by `angle`, to `b`'s. A
/// line's normal is its direction turned a quarter counter-clockwise; a circle's or arc's
/// points from `p` to its centre.
pub fn angle_via_point(
    s: &SketchSystem,
    v: &[f64],
    a: Geometry,
    b: Geometry,
    p: (f64, f64),
    angle: f64,
) -> f64 {
    let normal = |g: Geometry| match s.geometry[g.0 as usize] {
        SketchGeometry::Line { .. } => {
            let d = dir(s, v, g);
            (-d.1, d.0)
        }
        _ => {
            let o = xy(s, v, PointOf::Center(g));
            (o.0 - p.0, o.1 - p.1)
        }
    };
    let (n1, n2) = (normal(a), normal(b));
    let (c, sn) = (libm::cos(angle), libm::sin(angle));
    let r = (n1.0 * c - n1.1 * sn, n1.0 * sn + n1.1 * c);
    libm::atan2(-n2.0 * r.1 + n2.1 * r.0, n2.0 * r.0 + n2.1 * r.1)
}
