//! Random valid cases, for broad coverage: each seed gives the same cases every time.
//! Shapes are valid by construction (sizes well above tolerance, fillets and chamfers that
//! fit their box, booleans of shapes that overlap, profiles that are simple polygons), so a
//! case that fails on either side is worth a look. Values are rounded to a thousandth of a
//! millimetre (or degree), so a case reads as it runs.

use crate::case::{Case, Chamfer, Shape};

/// SplitMix64: small, fast, and the same everywhere.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `[low, high)`, rounded to a thousandth.
    fn between(&mut self, low: f64, high: f64) -> f64 {
        // The top 53 bits, a fraction in [0, 1) exactly.
        let fraction = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        thousandths(low + (high - low) * fraction)
    }

    /// An index below `n` (which must be above 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

fn thousandths(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Generated cases whose difference from FreeCAD is understood, as the corpus's are
/// ([`crate::corpus::Known`]): reported as known wherever a sweep makes them.
pub const KNOWN: &[crate::corpus::Known] = &[crate::corpus::Known {
    case: "generated_15_96",
    difference: "FreeCAD built it; Silico failed: invalid_result: Cut gave an invalid result, \
                 which was refused: the cut's result isn't a valid solid. Suggestion: faces or \
                 edges of the two shapes nearly meet: move one so they clearly overlap or stay \
                 clearly apart.",
    why: "a torus (R 12.531, r 9.057) less a torus (R 23.399, r 12.985) moved (-3.965, 0.164, \
          1.576): OCCT finds no intersection and takes the tool as inside the base, so \
          FreeCAD 1.1.3's cut is one solid turned inside out, volume -57587.21 (the base less \
          the whole tool), which its isValid and check pass; Silico refuses it as invalid. The \
          base within 6.446 of its axis is outside the tool: at least 908 mm³ is left (about \
          5854 by Monte Carlo) (docs/ports/booleans.md, Known wrong results)",
    platforms: &["aarch64-macos", "x86_64-linux"],
}];

/// `count` cases from `seed`, named `generated_<seed>_<n>`.
pub fn cases(seed: u64, count: usize) -> Vec<Case> {
    let mut random = Random(seed);
    (0..count)
        .map(|n| {
            let shape = match random.below(7) {
                0 => primitive(&mut random),
                1 => Shape::Translate {
                    shape: Box::new(primitive(&mut random)),
                    by: [
                        random.between(-50.0, 50.0),
                        random.between(-50.0, 50.0),
                        random.between(-50.0, 50.0),
                    ],
                },
                2 => boolean(&mut random),
                3 => fillet(&mut random),
                4 => chamfer(&mut random),
                5 => extrude(&mut random),
                _ => revolve(&mut random),
            };
            Case {
                name: format!("generated_{seed}_{n}"),
                note: "generated".to_owned(),
                boundary: None,
                shape,
            }
        })
        .collect()
}

fn primitive(random: &mut Random) -> Shape {
    match random.below(5) {
        0 => Shape::Block {
            dx: random.between(1.0, 40.0),
            dy: random.between(1.0, 40.0),
            dz: random.between(1.0, 40.0),
        },
        1 => Shape::Cylinder {
            radius: random.between(1.0, 20.0),
            height: random.between(1.0, 40.0),
        },
        2 => Shape::Sphere {
            radius: random.between(1.0, 20.0),
        },
        3 => {
            let radius1 = random.between(1.0, 20.0);
            // A point, or a radius at least a millimetre from the other.
            let radius2 = if random.coin() {
                0.0
            } else {
                thousandths(radius1 + random.between(1.0, 15.0))
            };
            Shape::Cone {
                radius1,
                radius2,
                height: random.between(1.0, 40.0),
            }
        }
        _ => {
            let major_radius = random.between(5.0, 30.0);
            Shape::Torus {
                major_radius,
                minor_radius: random.between(1.0, major_radius * 0.8),
            }
        }
    }
}

/// A boolean of two primitives, the tool moved by less than the size of either, so they
/// mostly overlap.
fn boolean(random: &mut Random) -> Shape {
    let base = Box::new(primitive(random));
    let tool = Box::new(Shape::Translate {
        shape: Box::new(primitive(random)),
        by: [
            random.between(-5.0, 5.0),
            random.between(-5.0, 5.0),
            random.between(-5.0, 5.0),
        ],
    });
    match random.below(3) {
        0 => Shape::Fuse { base, tool },
        1 => Shape::Cut { base, tool },
        _ => Shape::Common { base, tool },
    }
}

/// A box, and one to three of its twelve edges, distinct and in increasing order.
fn box_and_edges(random: &mut Random) -> (Shape, f64, Vec<u32>) {
    let (dx, dy, dz) = (
        random.between(5.0, 40.0),
        random.between(5.0, 40.0),
        random.between(5.0, 40.0),
    );
    let mut edges: Vec<u32> = (0..=random.below(3))
        .map(|_| u32::try_from(random.below(12)).unwrap_or(0) + 1)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    (Shape::Block { dx, dy, dz }, dx.min(dy).min(dz), edges)
}

/// A fillet of a quarter of the box's smallest side at most, which always fits.
fn fillet(random: &mut Random) -> Shape {
    let (block, smallest, edges) = box_and_edges(random);
    Shape::Fillet {
        shape: Box::new(block),
        edges,
        radius: random.between(0.05 * smallest, 0.25 * smallest),
    }
}

/// A chamfer whose sizes reach at most a quarter of the box's smallest side.
fn chamfer(random: &mut Random) -> Shape {
    let (block, smallest, edges) = box_and_edges(random);
    let distance = random.between(0.05 * smallest, 0.25 * smallest);
    let size = match random.below(3) {
        0 => Chamfer::Equal { distance },
        1 => Chamfer::TwoDistances {
            distance,
            other: random.between(0.05 * smallest, 0.25 * smallest),
        },
        // tan 45° is 1, so the cut reaches at most the distance along the other face.
        _ => Chamfer::DistanceAngle {
            distance,
            angle: random.between(20.0, 45.0),
        },
    };
    Shape::Chamfer {
        shape: Box::new(block),
        edges,
        size,
        flip: random.coin(),
    }
}

/// A rectangle, a triangle, or an L in the plane z = 0, counterclockwise from above.
fn profile(random: &mut Random) -> Vec<[f64; 3]> {
    let (w, h) = (random.between(2.0, 30.0), random.between(2.0, 30.0));
    match random.below(3) {
        0 => vec![[0.0, 0.0, 0.0], [w, 0.0, 0.0], [w, h, 0.0], [0.0, h, 0.0]],
        1 => {
            let apex = random.between(0.0, w);
            vec![[0.0, 0.0, 0.0], [w, 0.0, 0.0], [apex, h, 0.0]]
        }
        _ => {
            // The arm, from a third to two thirds of each side.
            let (a, b) = (
                thousandths(w * random.between(0.33, 0.66)),
                thousandths(h * random.between(0.33, 0.66)),
            );
            vec![
                [0.0, 0.0, 0.0],
                [w, 0.0, 0.0],
                [w, b, 0.0],
                [a, b, 0.0],
                [a, h, 0.0],
                [0.0, h, 0.0],
            ]
        }
    }
}

/// A profile swept up its normal, or up a direction less than 45° from it (x² + y² at most
/// 0.98, under z² = 1).
fn extrude(random: &mut Random) -> Shape {
    let profile = profile(random);
    let direction = random
        .coin()
        .then(|| [random.between(-0.7, 0.7), random.between(-0.7, 0.7), 1.0]);
    Shape::Extrude {
        profile,
        length: random.between(1.0, 30.0),
        direction,
    }
}

/// A rectangle in the plane y = 0, off the z axis or touching it, turned about it.
fn revolve(random: &mut Random) -> Shape {
    let inner = if random.coin() {
        0.0
    } else {
        random.between(1.0, 20.0)
    };
    let outer = thousandths(inner + random.between(1.0, 15.0));
    let (bottom, top) = (0.0, random.between(1.0, 30.0));
    Shape::Revolve {
        profile: vec![
            [inner, 0.0, bottom],
            [outer, 0.0, bottom],
            [outer, 0.0, top],
            [inner, 0.0, top],
        ],
        origin: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
        angle: random.between(5.0, 360.0),
    }
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn the_known_generated_case_is_the_torus_cut_it_says() {
        // generate::KNOWN's case, as seed 15 makes it: if the generator changes, its entry
        // is about another case and must be redone.
        let case = &cases(15, 97)[96];
        assert_eq!(case.name, super::KNOWN[0].case);
        let json = serde_json::to_value(&case.shape).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"cut": {
                "base": {"torus": {"major_radius": 12.531, "minor_radius": 9.057}},
                "tool": {"translate": {
                    "shape": {"torus": {"major_radius": 23.399, "minor_radius": 12.985}},
                    "by": [-3.965, 0.164, 1.576]
                }}
            }})
        );
    }

    #[test]
    fn a_seed_gives_the_same_cases_every_time() {
        assert_eq!(cases(7, 50), cases(7, 50));
        assert_ne!(cases(7, 50), cases(8, 50));
    }

    #[test]
    fn every_kind_of_case_comes_up() {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let generated = cases(1, 300);
        for case in &generated {
            seen.insert(case.shape.operation());
        }
        for operation in [
            "box",
            "cylinder",
            "sphere",
            "cone",
            "torus",
            "translate",
            "fuse",
            "cut",
            "common",
            "fillet",
            "chamfer",
            "extrude",
            "revolve",
        ] {
            assert!(seen.contains(operation), "no {operation} in 300 cases");
        }
    }
}

/// `count` sketch-edit cases from `seed`, named `edit_<seed>_<n>`: each a random sketch,
/// drawn exactly as it solves (its constraints hold at its coordinates: axis-aligned lines,
/// lengths and radii as drawn, an angle of 45°, points on lines at their coordinates), and a
/// random edit of it. The sketch: a rectangle, some of whose sides are horizontal or
/// vertical, its corner placed or not; a tail line from one corner, with an arc tangent at
/// its end (a three-way joint and a tangent_at); lines crossing the rectangle, upright,
/// across, and at 45°; circles crossing or tangent to its bottom, one equal to another, a
/// line tangent on one; points on its bottom, symmetric about it, and at its middle; and a
/// line starting at a separate point where a corner is (ends a merge joins).
pub fn sketch_edit_cases(
    seed: u64,
    count: usize,
) -> Result<Vec<crate::sketch_edit::EditCase>, String> {
    let mut random = Random(seed);
    (0..count)
        .map(|n| {
            let name = format!("edit_{seed}_{n}");
            let drawing = Drawing::random(&mut random);
            let edit = drawing.edit(&mut random);
            let text = drawing.document(&name);
            let document = silico_core::document::read(text.as_bytes())
                .map_err(|e| format!("{name}: the generated sketch can't be read: {e}"))?;
            silico_core::document::validate(&document).map_err(|invalid| {
                let errors: Vec<String> = invalid.errors().iter().map(|e| e.to_string()).collect();
                format!(
                    "{name}: the gate refuses the generated sketch: {}",
                    errors.join("; ")
                )
            })?;
            Ok(crate::sketch_edit::EditCase {
                name,
                note: "generated".to_owned(),
                document,
                feature: 0,
                edit,
            })
        })
        .collect()
}

/// A sketch being drawn: its members as the file writes them.
#[derive(Default)]
struct Drawing {
    points: Vec<(String, [f64; 2])>,
    /// Lines: ID, from, to.
    lines: Vec<(String, String, String)>,
    /// Circles: ID, centre, radius.
    circles: Vec<(String, String, f64)>,
    /// Arcs: ID, centre, start, end.
    arcs: Vec<(String, String, String, String)>,
    /// Each constraint's members after its ID, as JSON.
    constraints: Vec<String>,
    /// Pairs of separate points at one place.
    touching: Vec<(String, String)>,
    next: u32,
}

impl Drawing {
    fn id(&mut self, prefix: char) -> String {
        self.next += 1;
        format!("{prefix}{:7}", self.next)
    }

    fn point(&mut self, at: [f64; 2]) -> String {
        let id = self.id('p');
        self.points.push((id.clone(), at));
        id
    }

    fn at(&self, point: &str) -> [f64; 2] {
        self.points
            .iter()
            .find(|(id, _)| id == point)
            .map_or([0.0, 0.0], |(_, at)| *at)
    }

    fn line(&mut self, from: &str, to: &str) -> String {
        let id = self.id('n');
        self.lines
            .push((id.clone(), from.to_owned(), to.to_owned()));
        id
    }

    fn circle(&mut self, center: &str, radius: f64) -> String {
        let id = self.id('c');
        self.circles.push((id.clone(), center.to_owned(), radius));
        id
    }

    fn arc(&mut self, center: &str, start: &str, end: &str) -> String {
        let id = self.id('a');
        self.arcs.push((
            id.clone(),
            center.to_owned(),
            start.to_owned(),
            end.to_owned(),
        ));
        id
    }

    fn constrain(&mut self, body: String) {
        let id = self.id('k');
        self.constraints.push(format!("\"id\": \"{id}\", {body}"));
    }

    fn random(random: &mut Random) -> Drawing {
        let mut d = Drawing::default();
        let maybe =
            |random: &mut Random, p: f64| (random.next() >> 11) as f64 / (1u64 << 53) as f64 <= p;
        let (x0, y0) = (random.between(-30.0, 0.0), random.between(-20.0, 0.0));
        let (w, h) = (random.between(10.0, 40.0), random.between(8.0, 30.0));
        let (x1, y1) = (x0 + w, y0 + h);
        let c0 = d.point([x0, y0]);
        let c1 = d.point([x1, y0]);
        let c2 = d.point([x1, y1]);
        let c3 = d.point([x0, y1]);
        let bottom = d.line(&c0, &c1);
        let right = d.line(&c1, &c2);
        let top = d.line(&c2, &c3);
        let left = d.line(&c3, &c0);
        let bottom_horizontal = maybe(random, 0.7);
        if bottom_horizontal {
            d.constrain(format!("\"type\": \"horizontal\", \"line\": \"{bottom}\""));
        }
        for (line, kind) in [
            (&right, "vertical"),
            (&top, "horizontal"),
            (&left, "vertical"),
        ] {
            if maybe(random, 0.5) {
                d.constrain(format!("\"type\": \"{kind}\", \"line\": \"{line}\""));
            }
        }
        if maybe(random, 0.5) {
            d.constrain(format!(
                "\"type\": \"length\", \"line\": \"{bottom}\", \"value\": {}, \"driving\": {}",
                thousandths(x1 - x0),
                maybe(random, 0.8)
            ));
        }
        if maybe(random, 0.5) {
            d.constrain(format!(
                "\"type\": \"distance_x\", \"a\": \"origin\", \"b\": \"{c0}\", \"value\": {x0}, \"driving\": true"
            ));
            d.constrain(format!(
                "\"type\": \"distance_y\", \"a\": \"origin\", \"b\": \"{c0}\", \"value\": {y0}, \"driving\": true"
            ));
        }
        // A tail from the bottom-right corner, and an arc on at its end.
        if maybe(random, 0.6) {
            let length = random.between(4.0, 15.0);
            let end = d.point([x1 + length, y0]);
            let tail = d.line(&c1, &end);
            if bottom_horizontal && maybe(random, 0.5) {
                d.constrain(format!(
                    "\"type\": \"parallel\", \"a\": \"{bottom}\", \"b\": \"{tail}\""
                ));
            }
            if maybe(random, 0.6) {
                let r = random.between(2.0, 8.0);
                let center = d.point([x1 + length, y0 + r]);
                let arc_end = d.point([x1 + length + r, y0 + r]);
                let arc = d.arc(&center, &end, &arc_end);
                if maybe(random, 0.7) {
                    d.constrain(format!(
                        "\"type\": \"tangent_at\", \"a\": \"{tail}\", \"b\": \"{arc}\", \"at\": \"{end}\", \"direction\": \"same\""
                    ));
                }
                if maybe(random, 0.5) {
                    d.constrain(format!(
                        "\"type\": \"radius\", \"curve\": \"{arc}\", \"value\": {r}, \"driving\": true"
                    ));
                }
            }
        }
        // An upright line across the bottom and top.
        if maybe(random, 0.7) {
            let x = thousandths(x0 + w * random.between(0.2, 0.8));
            let a = d.point([x, y0 - random.between(1.0, 6.0)]);
            let b = d.point([x, y1 + random.between(1.0, 6.0)]);
            let upright = d.line(&a, &b);
            if maybe(random, 0.5) {
                d.constrain(format!("\"type\": \"vertical\", \"line\": \"{upright}\""));
            } else if maybe(random, 0.5) {
                d.constrain(format!(
                    "\"type\": \"perpendicular\", \"a\": \"{bottom}\", \"b\": \"{upright}\""
                ));
            }
        }
        // A line across the left and right sides.
        if maybe(random, 0.5) {
            let y = thousandths(y0 + h * random.between(0.2, 0.8));
            let a = d.point([x0 - random.between(1.0, 6.0), y]);
            let b = d.point([x1 + random.between(1.0, 6.0), y]);
            let across = d.line(&a, &b);
            if maybe(random, 0.5) {
                d.constrain(format!("\"type\": \"horizontal\", \"line\": \"{across}\""));
            }
        }
        // A line at 45° across the bottom.
        if maybe(random, 0.5) {
            let x = thousandths(x0 + w * random.between(0.1, 0.5));
            let below = random.between(1.0, 5.0);
            let rise = random.between(below + 2.0, below + h);
            let a = d.point([x, y0 - below]);
            let b = d.point([x + rise, y0 - below + rise]);
            let slant = d.line(&a, &b);
            if maybe(random, 0.4) {
                d.constrain(format!(
                    "\"type\": \"angle\", \"a\": \"{bottom}\", \"b\": \"{slant}\", \"value\": 45, \"driving\": true"
                ));
            }
        }
        // A circle crossing the bottom, or tangent to it from inside.
        let mut circles = Vec::new();
        if maybe(random, 0.6) {
            let r = random.between(2.0, (h / 2.0).min(w / 3.0).max(2.5));
            let tangent = maybe(random, 0.4);
            let cx = thousandths(x0 + w * random.between(0.3, 0.7));
            let cy = if tangent {
                y0 + r
            } else {
                y0 + thousandths(r * random.between(-0.6, 0.6))
            };
            let center = d.point([cx, cy]);
            let circle = d.circle(&center, r);
            circles.push((circle.clone(), cx, cy, r));
            if tangent && maybe(random, 0.7) {
                d.constrain(format!(
                    "\"type\": \"tangent\", \"a\": \"{bottom}\", \"b\": \"{circle}\", \"side\": \"left\""
                ));
            }
            if maybe(random, 0.5) {
                d.constrain(format!(
                    "\"type\": \"radius\", \"curve\": \"{circle}\", \"value\": {r}, \"driving\": true"
                ));
            }
        }
        // A second circle of the same radius, crossing the right side, sometimes equal.
        if let Some((first, _, _, r)) = circles.first().cloned()
            && maybe(random, 0.4)
        {
            let center = d.point([x1, thousandths(y0 + h * random.between(0.3, 0.7))]);
            let circle = d.circle(&center, r);
            if maybe(random, 0.6) {
                d.constrain(format!(
                    "\"type\": \"equal\", \"a\": \"{first}\", \"b\": \"{circle}\""
                ));
            }
            circles.push((circle, x1, 0.0, r));
        }
        // A line leaving the top of the first circle, tangent on it.
        if let Some((circle, cx, cy, r)) = circles.first().cloned()
            && maybe(random, 0.3)
        {
            let a = d.point([cx, cy + r]);
            let b = d.point([cx + random.between(3.0, 10.0), cy + r]);
            let leaving = d.line(&a, &b);
            d.constrain(format!(
                "\"type\": \"tangent_on\", \"a\": \"{leaving}\", \"at\": \"{a}\", \"b\": \"{circle}\", \"direction\": \"opposite\""
            ));
        }
        // Points on the bottom, symmetric about it, at its middle, and loose.
        if maybe(random, 0.6) {
            let p = d.point([thousandths(x0 + w * random.between(0.1, 0.9)), y0]);
            if maybe(random, 0.8) {
                d.constrain(format!(
                    "\"type\": \"on\", \"point\": \"{p}\", \"curve\": \"{bottom}\""
                ));
            }
        }
        if maybe(random, 0.3) {
            let x = thousandths(x0 + w * random.between(0.1, 0.9));
            let e = random.between(1.0, 4.0);
            let a = d.point([x, y0 + e]);
            let b = d.point([x, y0 - e]);
            d.constrain(format!(
                "\"type\": \"symmetric\", \"a\": \"{a}\", \"b\": \"{b}\", \"about\": \"{bottom}\""
            ));
        }
        if maybe(random, 0.3) {
            let m = d.point([(x0 + x1) / 2.0, y0]);
            d.constrain(format!(
                "\"type\": \"midpoint\", \"point\": \"{m}\", \"line\": \"{bottom}\""
            ));
        }
        if maybe(random, 0.3) {
            d.point([random.between(-40.0, 40.0), random.between(-40.0, 40.0)]);
        }
        // A line from a separate point at the top-left corner.
        if maybe(random, 0.5) {
            let u = d.point([x0, y1]);
            let v = d.point([x0 - random.between(2.0, 8.0), y1 + random.between(2.0, 8.0)]);
            d.line(&u, &v);
            d.touching.push((c3, u));
        }
        d
    }

    /// The curves and what each is.
    fn curves(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|l| l.0.clone())
            .chain(self.circles.iter().map(|c| c.0.clone()))
            .chain(self.arcs.iter().map(|a| a.0.clone()))
            .collect()
    }

    /// The point at `t` (0 to 1) along `curve`, or for a circle at `t` of a turn.
    fn along(&self, curve: &str, t: f64) -> [f64; 2] {
        if let Some((_, from, to)) = self.lines.iter().find(|l| l.0 == curve) {
            let (a, b) = (self.at(from), self.at(to));
            return [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
        }
        if let Some((_, center, r)) = self.circles.iter().find(|c| c.0 == curve) {
            let c = self.at(center);
            let angle = t * std::f64::consts::TAU;
            return [c[0] + r * libm::cos(angle), c[1] + r * libm::sin(angle)];
        }
        if let Some((_, center, start, end)) = self.arcs.iter().find(|a| a.0 == curve) {
            let (c, s, e) = (self.at(center), self.at(start), self.at(end));
            if let Some([r, from, to]) = silico_protocol::arc_from_points(c, s, e) {
                let angle = from + t * (to - from);
                return [c[0] + r * libm::cos(angle), c[1] + r * libm::sin(angle)];
            }
        }
        [0.0, 0.0]
    }

    /// The points two or more curves use, each with those curves.
    fn shared(&self) -> Vec<(String, Vec<String>)> {
        let mut users: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
        for (id, from, to) in &self.lines {
            users.entry(from).or_default().push(id.clone());
            users.entry(to).or_default().push(id.clone());
        }
        for (id, center, _) in &self.circles {
            users.entry(center).or_default().push(id.clone());
        }
        for (id, center, start, end) in &self.arcs {
            for p in [center, start, end] {
                users.entry(p).or_default().push(id.clone());
            }
        }
        users
            .into_iter()
            .filter(|(_, curves)| curves.len() > 1)
            .map(|(p, curves)| (p.to_owned(), curves))
            .collect()
    }

    fn edit(&self, random: &mut Random) -> crate::sketch_edit::Edit {
        use crate::sketch_edit::Edit;
        let curves = self.curves();
        let pick = |random: &mut Random, list: &[String]| list[random.below(list.len())].clone();
        let is_circle = |c: &str| self.circles.iter().any(|x| x.0 == c);
        match random.below(100) {
            0..30 => {
                let curve = pick(random, &curves);
                let t = random.between(0.15, 0.85);
                let at = if is_circle(&curve) {
                    let u = t + random.between(0.15, 0.85);
                    vec![self.along(&curve, t), self.along(&curve, u.fract())]
                } else {
                    vec![self.along(&curve, t)]
                };
                Edit::Split { curve, at }
            }
            30..60 => {
                let curve = pick(random, &curves);
                let pick = self.along(&curve, random.between(0.05, 0.95));
                Edit::Trim { curve, pick }
            }
            60..75 => {
                let members = match random.below(10) {
                    0..7 => vec![pick(random, &curves)],
                    7..9 if !self.constraints.is_empty() => {
                        let c = &self.constraints[random.below(self.constraints.len())];
                        vec![c[7..15].to_owned()]
                    }
                    _ => vec![pick(random, &curves), pick(random, &curves)],
                };
                Edit::Remove { members }
            }
            75..87 => {
                let shared = self.shared();
                if shared.is_empty() {
                    return Edit::Remove {
                        members: vec![pick(random, &curves)],
                    };
                }
                let (point, users) = &shared[random.below(shared.len())];
                Edit::Unjoin {
                    point: point.clone(),
                    curve: pick(random, users),
                }
            }
            _ => {
                if let Some((a, b)) = self.touching.first()
                    && random.below(10) < 7
                {
                    let points = if random.coin() {
                        vec![a.clone(), b.clone()]
                    } else {
                        vec![b.clone(), a.clone()]
                    };
                    return Edit::Merge { points };
                }
                let ids: Vec<String> = self.points.iter().map(|p| p.0.clone()).collect();
                let a = pick(random, &ids);
                let b = pick(random, &ids);
                Edit::Merge { points: vec![a, b] }
            }
        }
    }

    /// The document of this one sketch, named `name`.
    fn document(&self, name: &str) -> String {
        let points: Vec<String> = self
            .points
            .iter()
            .map(|(id, [x, y])| {
                format!("{{\"id\": \"{id}\", \"x\": {x}, \"y\": {y}, \"construction\": false}}")
            })
            .collect();
        let lines: Vec<String> = self
            .lines
            .iter()
            .map(|(id, from, to)| {
                format!("{{\"id\": \"{id}\", \"from\": \"{from}\", \"to\": \"{to}\", \"construction\": false}}")
            })
            .collect();
        let circles: Vec<String> = self
            .circles
            .iter()
            .map(|(id, center, r)| {
                format!("{{\"id\": \"{id}\", \"center\": \"{center}\", \"radius\": {r}, \"construction\": false}}")
            })
            .collect();
        let arcs: Vec<String> = self
            .arcs
            .iter()
            .map(|(id, center, start, end)| {
                format!("{{\"id\": \"{id}\", \"center\": \"{center}\", \"start\": \"{start}\", \"end\": \"{end}\", \"construction\": false}}")
            })
            .collect();
        let constraints: Vec<String> = self
            .constraints
            .iter()
            .map(|c| format!("{{{c}}}"))
            .collect();
        format!(
            "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"s0000001\", \"type\": \"sketch\", \"v\": 1, \"name\": \"{name}\", \"plane\": \"xy\", \"points\": [{}], \"lines\": [{}], \"circles\": [{}], \"arcs\": [{}], \"constraints\": [{}]}}]}}",
            points.join(", "),
            lines.join(", "),
            circles.join(", "),
            arcs.join(", "),
            constraints.join(", ")
        )
    }
}
