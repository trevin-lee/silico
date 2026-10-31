//! For tests of a sketch's profile: a sketch drawn point by point, solved as drawn (the
//! solution holds the stored coordinates), and its profile or its refusals.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use silico_core::document::{
    Checked, Coordinate, Document, ElementId, Feature, FeatureId, FeatureVersion, FormatVersion,
    Invalid, Name, NamingScheme, Operation, Plane, Sketch, SketchArc, SketchCircle, SketchLine,
    SketchPoint, Units,
};
use silico_core::profile::{Profile, Refused, profile};
use silico_core::sketch::SketchSolution;
use silico_core::{Error, ErrorCode};
use silico_protocol::Algorithm;

/// The sketch feature's ID.
pub const FEATURE: &str = "k7f2qx3m";

/// A sketch being drawn. IDs are the kind's letter (p points, n lines, c circles, a arcs)
/// and a count, so they sort in the order drawn within a kind.
pub struct Draw {
    pub sketch: Sketch,
    count: usize,
}

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap_or_else(|| panic!("{text} isn't an element ID"))
}

fn coordinate(v: f64) -> Coordinate {
    Coordinate::new(v).unwrap()
}

impl Default for Draw {
    fn default() -> Self {
        Self::new()
    }
}

impl Draw {
    pub fn new() -> Self {
        Draw {
            sketch: Sketch {
                plane: Plane::Xy,
                points: Vec::new(),
                lines: Vec::new(),
                circles: Vec::new(),
                arcs: Vec::new(),
                constraints: Vec::new(),
            },
            count: 0,
        }
    }

    fn next(&mut self, kind: char) -> String {
        self.count += 1;
        format!("{kind}{:07}", self.count)
    }

    pub fn point(&mut self, x: f64, y: f64) -> String {
        let name = self.next('p');
        self.point_named(&name, x, y)
    }

    pub fn point_named(&mut self, name: &str, x: f64, y: f64) -> String {
        self.sketch.points.push(SketchPoint {
            id: id(name),
            x: coordinate(x),
            y: coordinate(y),
            construction: false,
        });
        name.to_owned()
    }

    pub fn line(&mut self, from: &str, to: &str) -> String {
        let name = self.next('n');
        self.line_named(&name, from, to)
    }

    pub fn line_named(&mut self, name: &str, from: &str, to: &str) -> String {
        self.sketch.lines.push(SketchLine {
            id: id(name),
            from: id(from),
            to: id(to),
            construction: false,
        });
        name.to_owned()
    }

    pub fn construction_line(&mut self, from: &str, to: &str) -> String {
        let name = self.line(from, to);
        if let Some(l) = self.sketch.lines.last_mut() {
            l.construction = true;
        }
        name
    }

    /// A circle about a new point at (x, y).
    pub fn circle(&mut self, x: f64, y: f64, radius: f64) -> String {
        let center = self.point(x, y);
        self.circle_about(&center, radius)
    }

    pub fn circle_about(&mut self, center: &str, radius: f64) -> String {
        let name = self.next('c');
        self.sketch.circles.push(SketchCircle {
            id: id(&name),
            center: id(center),
            radius: coordinate(radius),
            construction: false,
        });
        name
    }

    /// An arc about `center`, counter-clockwise from `start` to `end`.
    pub fn arc(&mut self, center: &str, start: &str, end: &str) -> String {
        let name = self.next('a');
        self.sketch.arcs.push(SketchArc {
            id: id(&name),
            center: id(center),
            start: id(start),
            end: id(end),
            construction: false,
        });
        name
    }

    /// A closed polygon through new points at `corners`, in order: its lines, each from a
    /// corner to the next.
    pub fn polygon(&mut self, corners: &[(f64, f64)]) -> Vec<String> {
        let points: Vec<String> = corners.iter().map(|&(x, y)| self.point(x, y)).collect();
        (0..points.len())
            .map(|i| self.line(&points[i].clone(), &points[(i + 1) % points.len()].clone()))
            .collect()
    }

    /// A rectangle from (x0, y0) to (x1, y1), drawn counter-clockwise from (x0, y0) (or
    /// clockwise if `x0 > x1` or `y0 > y1` but not both).
    pub fn rectangle(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<String> {
        self.polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
    }

    /// The solution that leaves everything where it's drawn.
    pub fn solution(&self) -> SketchSolution {
        let points: BTreeMap<ElementId, [Coordinate; 2]> = self
            .sketch
            .points
            .iter()
            .map(|p| (p.id.clone(), [p.x, p.y]))
            .collect();
        let radii = self
            .sketch
            .circles
            .iter()
            .map(|c| (c.id.clone(), c.radius))
            .collect();
        SketchSolution {
            points,
            radii,
            references: BTreeMap::new(),
            dofs: 0,
            free: BTreeSet::new(),
            redundant: BTreeMap::new(),
            partially_redundant: BTreeMap::new(),
            algorithm: Algorithm::DogLeg,
        }
    }

    /// The document of this one sketch, through the gate.
    pub fn checked(&self) -> Result<Checked, Invalid> {
        Checked::new(Document {
            silico: FormatVersion,
            units: Units,
            naming: NamingScheme,
            features: vec![Feature {
                id: feature(),
                operation: Operation::Sketch(self.sketch.clone()),
                version: FeatureVersion,
                name: Name::new("Sketch").unwrap(),
                suppressed: false,
                note: None,
                fingerprint: None,
            }],
        })
    }

    /// The document of this one sketch, stored with its points spread 10 mm apart on a grid
    /// and its circles of radius 1, through the gate: its elements and how they join, as
    /// drawn, with nothing the gate refuses as too small. The profile takes the sketch's
    /// shape from the solution alone, so this is how its own rules are tested on what a
    /// solve might give, the gate's floor aside (a solve under it isn't kept).
    pub fn structure(&self) -> Result<Checked, Invalid> {
        let mut sketch = self.sketch.clone();
        for (k, point) in sketch.points.iter_mut().enumerate() {
            point.x = coordinate(10.0 * (k % 100) as f64 + 10.0);
            point.y = coordinate(10.0 * (k / 100) as f64 + 10.0);
        }
        for circle in &mut sketch.circles {
            circle.radius = coordinate(1.0);
        }
        Draw { sketch, count: 0 }.checked()
    }

    /// The profile from `solution`, of the sketch's structure the gate takes.
    pub fn profile_from(&self, solution: &SketchSolution) -> Result<Profile, Refused> {
        let checked = match self.structure() {
            Ok(c) => c,
            Err(invalid) => panic!("the gate refuses the sketch: {:#?}", invalid.errors()),
        };
        profile(&checked, &feature(), solution)
    }

    /// The profile, solved where it's drawn.
    pub fn profile(&self) -> Result<Profile, Refused> {
        self.profile_from(&self.solution())
    }

    /// The profile, which must be one.
    pub fn made(&self) -> Profile {
        match self.profile() {
            Ok(p) => p,
            Err(r) => panic!("refused: {:#?}", messages(&r)),
        }
    }

    /// The refusals, which there must be, each checked for its code, feature, and count.
    pub fn refused(&self) -> Vec<Error> {
        match self.profile() {
            Ok(p) => panic!("made a profile: {p:#?}"),
            Err(r) => {
                for e in r.errors() {
                    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e}");
                    assert_eq!(e.feature(), Some(FEATURE), "{e}");
                    assert_eq!(e.elements_total(), e.elements().len() as u64, "{e}");
                    assert!(e.suggestion().is_some(), "{e}");
                }
                assert_eq!(r.total(), r.errors().len() as u64);
                // Every refusal listed: the elements at fault are theirs, all of them.
                let union: BTreeSet<String> = r
                    .errors()
                    .iter()
                    .flat_map(|e| e.elements().iter().cloned())
                    .collect();
                assert_eq!(r.elements_total(), union.len() as u64);
                let listed: Vec<String> = union.into_iter().take(100).collect();
                assert_eq!(r.elements(), listed);
                r.errors().to_vec()
            }
        }
    }
}

pub fn feature() -> FeatureId {
    FeatureId::new(FEATURE).unwrap()
}

pub fn messages(r: &Refused) -> Vec<String> {
    r.errors().iter().map(ToString::to_string).collect()
}

/// Each refusal's message and elements.
pub fn named(errors: &[Error]) -> Vec<(String, Vec<String>)> {
    errors
        .iter()
        .map(|e| (e.message().to_owned(), e.elements().to_vec()))
        .collect()
}

/// Whether `a` and `b` agree to `tolerance`, relative to the larger of 1 and |b|.
pub fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance * b.abs().max(1.0)
}
