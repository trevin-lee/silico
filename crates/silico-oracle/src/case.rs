//! Oracle cases: how to build a shape from Silico's operations, as JSON (one file holds a
//! list of cases). Shapes nest: a fuse names its two shapes, a fillet the shape it rounds.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One oracle case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// Unique across the cases run together.
    pub name: String,
    /// What the case checks.
    pub note: String,
    /// The tolerance boundary the case sits near, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<String>,
    pub shape: Shape,
}

/// How to build a shape. Lengths are in millimetres and angles in degrees, as Silico's
/// operations take them; edges are numbered from 1, as `Op::Fillet` numbers them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    #[serde(rename = "box")]
    Block {
        dx: f64,
        dy: f64,
        dz: f64,
    },
    Cylinder {
        radius: f64,
        height: f64,
    },
    Sphere {
        radius: f64,
    },
    Cone {
        radius1: f64,
        radius2: f64,
        height: f64,
    },
    Torus {
        major_radius: f64,
        minor_radius: f64,
    },
    Translate {
        shape: Box<Shape>,
        by: [f64; 3],
    },
    Fuse {
        base: Box<Shape>,
        tool: Box<Shape>,
    },
    Cut {
        base: Box<Shape>,
        tool: Box<Shape>,
    },
    Common {
        base: Box<Shape>,
        tool: Box<Shape>,
    },
    Fillet {
        shape: Box<Shape>,
        edges: Vec<u32>,
        radius: f64,
    },
    Chamfer {
        shape: Box<Shape>,
        edges: Vec<u32>,
        size: Chamfer,
        #[serde(default)]
        flip: bool,
    },
    /// A flat polygon swept along its normal (the right-hand rule of its corners' order) by
    /// `length`, or along `direction` far enough to rise `length` along the normal.
    Extrude {
        profile: Vec<[f64; 3]>,
        length: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<[f64; 3]>,
    },
    /// A flat polygon turned `angle` degrees about the axis through `origin` along
    /// `direction`.
    Revolve {
        profile: Vec<[f64; 3]>,
        origin: [f64; 3],
        direction: [f64; 3],
        angle: f64,
    },
}

/// A chamfer's sizes, as `ChamferSize` has them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Chamfer {
    Equal { distance: f64 },
    TwoDistances { distance: f64, other: f64 },
    DistanceAngle { distance: f64, angle: f64 },
}

impl Shape {
    /// The operation that makes the shape last: `box`, `fuse`, `fillet`, ...
    pub fn operation(&self) -> &'static str {
        match self {
            Shape::Block { .. } => "box",
            Shape::Cylinder { .. } => "cylinder",
            Shape::Sphere { .. } => "sphere",
            Shape::Cone { .. } => "cone",
            Shape::Torus { .. } => "torus",
            Shape::Translate { .. } => "translate",
            Shape::Fuse { .. } => "fuse",
            Shape::Cut { .. } => "cut",
            Shape::Common { .. } => "common",
            Shape::Fillet { .. } => "fillet",
            Shape::Chamfer { .. } => "chamfer",
            Shape::Extrude { .. } => "extrude",
            Shape::Revolve { .. } => "revolve",
        }
    }
}

/// Every case in the JSON files directly in `dir`, in file-name order, each file a list of
/// cases. Fails on a file that isn't one, and on two cases with one name.
pub fn load(dir: &Path) -> Result<Vec<Case>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("can't read {}: {e}", dir.display()))?;
    let mut files = BTreeSet::new();
    for entry in entries {
        let path = entry
            .map_err(|e| format!("can't read {}: {e}", dir.display()))?
            .path();
        if path.extension().is_some_and(|e| e == "json") {
            files.insert(path);
        }
    }
    let mut cases: Vec<Case> = Vec::new();
    let mut names = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("can't read {}: {e}", file.display()))?;
        let these: Vec<Case> = serde_json::from_str(&text)
            .map_err(|e| format!("{} isn't a list of cases: {e}", file.display()))?;
        for case in these {
            if !names.insert(case.name.clone()) {
                return Err(format!(
                    "two cases are named {} ({})",
                    case.name,
                    file.display()
                ));
            }
            cases.push(case);
        }
    }
    Ok(cases)
}
