//! Drag latency (docs/roadmap/m3-sketcher.md, M3.7): one frame's round trip while a point
//! is dragged, through the sketch worker and in this process, against the 16.7 ms a frame
//! has at 60 Hz. Recorded, never budgeted: it fails on no time. Run with
//! `pixi run cargo bench -p silico-sketch --bench drag`.
//!
//! Each sketch is under-constrained so a drag moves something: a chain of links whose
//! lengths and every other angle are fixed, its last end free; and a grid of rectangles,
//! the last one's size left free. Sizes run from the stress corpus's (tens of constraints)
//! to the worker's limit (2,048 unknowns). The dragged point follows a smooth path, 300
//! steps of 0.05 mm along a circle of radius 10 mm, each frame timed four ways:
//!
//! - `solve`, today's path: a full solve through the worker each frame (built and
//!   diagnosed each time), the point fixed at the cursor, from the last frame's values;
//! - `drag`: a drag session through the worker, begun once (timed apart), then a move per
//!   frame;
//! - `solve-here` and `drag-here`: the same, called in this process, so the difference is
//!   what the worker's IPC costs.
//!
//! A mode stops early, after at least 30 frames, once it has taken a minute: a full solve
//! at the largest sizes takes seconds a frame. The table says how many frames each ran.
//! Every row is printed again as a JSON line after the table, and written to the file
//! `SILICO_DRAG_BENCH_OUT` names, if it's set.

use std::error::Error;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Algorithm, DragBegin, Generation, Geometry, Op, PointOf, PointValues, Reply, SketchGeometry,
    SketchSystem, SolveStatus, SolverConstraint, Tag, Value,
};
use silico_sketch::Drag;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const FRAMES: usize = 300;
const STEP: f64 = 0.05;
const PATH_RADIUS: f64 = 10.0;
const MIN_FRAMES: usize = 30;
const MODE_BUDGET: Duration = Duration::from_secs(60);
const FRAME_BUDGET: Duration = Duration::from_micros(16_667);
const TIMEOUT: Duration = Duration::from_secs(600);

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Builds a system value by value.
#[derive(Default)]
struct Builder {
    values: Vec<f64>,
    unknowns: Vec<Value>,
    geometry: Vec<SketchGeometry>,
    constraints: Vec<SolverConstraint>,
}

impl Builder {
    fn fixed(&mut self, v: f64) -> Value {
        self.values.push(v);
        Value((self.values.len() - 1) as u32)
    }

    fn unknown(&mut self, v: f64) -> Value {
        let at = self.fixed(v);
        self.unknowns.push(at);
        at
    }

    fn line(&mut self, a: (f64, f64), b: (f64, f64)) -> Geometry {
        let start = PointValues {
            x: self.unknown(a.0),
            y: self.unknown(a.1),
        };
        let end = PointValues {
            x: self.unknown(b.0),
            y: self.unknown(b.1),
        };
        self.geometry.push(SketchGeometry::Line { start, end });
        Geometry((self.geometry.len() - 1) as u32)
    }

    fn fixed_point(&mut self, x: f64, y: f64) -> Geometry {
        let p = PointValues {
            x: self.fixed(x),
            y: self.fixed(y),
        };
        self.geometry.push(SketchGeometry::Point(p));
        Geometry((self.geometry.len() - 1) as u32)
    }

    fn constrain(&mut self, c: impl FnOnce(Tag) -> SolverConstraint) {
        let tag = Tag(self.constraints.len() as u32 + 1);
        self.constraints.push(c(tag));
    }

    fn system(self) -> SketchSystem {
        SketchSystem {
            values: self.values,
            unknowns: self.unknowns,
            geometry: self.geometry,
            constraints: self.constraints,
            algorithms: vec![
                Algorithm::DogLeg,
                Algorithm::LevenbergMarquardt,
                Algorithm::Bfgs,
            ],
        }
    }
}

/// A sketch to drag: its system, solved, and the point dragged.
struct Case {
    name: String,
    system: SketchSystem,
    point: PointOf,
}

/// `links` lines 10 long, joined end to start from a fixed point at the origin, every
/// other one at 30° to the one before, drawn roughly along +x; the last end dragged.
fn chain(links: usize) -> (SketchSystem, PointOf) {
    let mut b = Builder::default();
    let origin = b.fixed_point(0.0, 0.0);
    let ten = b.fixed(10.0);
    let angle = b.fixed(std::f64::consts::FRAC_PI_6);
    let mut before: Option<Geometry> = None;
    for i in 0..links {
        let x = 10.0 * i as f64;
        let line = b.line((x + 0.1, 0.2), (x + 9.7, -0.3));
        b.constrain(|tag| SolverConstraint::Distance {
            tag,
            a: PointOf::Start(line),
            b: PointOf::End(line),
            value: ten,
        });
        let joined = match before {
            None => PointOf::Point(origin),
            Some(before) => PointOf::End(before),
        };
        b.constrain(|tag| SolverConstraint::Coincident {
            tag,
            a: joined,
            b: PointOf::Start(line),
        });
        if let (Some(before), 1) = (before, i % 2) {
            b.constrain(|tag| SolverConstraint::Angle {
                tag,
                a: before,
                b: line,
                value: angle,
            });
        }
        before = Some(line);
    }
    let last = before.map_or(PointOf::Point(origin), PointOf::End);
    (b.system(), last)
}

/// `n` × `n` rectangles 8 by 6, 12 apart, each joined, horizontal and vertical, and its
/// bottom-left corner placed from its neighbour's; the first pinned to the origin. Every
/// size fixed but the last rectangle's, whose top-right corner is dragged.
fn grid(n: usize) -> (SketchSystem, PointOf) {
    let mut b = Builder::default();
    let origin = b.fixed_point(0.0, 0.0);
    let (w, h, gap) = (b.fixed(8.0), b.fixed(6.0), b.fixed(12.0));
    let zero = b.fixed(0.0);
    // Each rectangle's bottom side, by row; and the last one's right side.
    let mut rows: Vec<Vec<Geometry>> = Vec::new();
    let mut dragged = Geometry(0);
    for i in 0..n {
        let mut row = Vec::new();
        for j in 0..n {
            let (x, y) = (12.0 * j as f64, 12.0 * i as f64);
            let corners = [(x, y), (x + 8.0, y), (x + 8.0, y + 6.0), (x, y + 6.0)];
            let sides: Vec<Geometry> = (0..4)
                .map(|k| b.line(corners[k], corners[(k + 1) % 4]))
                .collect();
            for k in 0..4 {
                let (a, next) = (sides[k], sides[(k + 1) % 4]);
                b.constrain(|tag| SolverConstraint::Coincident {
                    tag,
                    a: PointOf::End(a),
                    b: PointOf::Start(next),
                });
            }
            let (bottom, right, top, left) = (sides[0], sides[1], sides[2], sides[3]);
            b.constrain(|tag| SolverConstraint::Horizontal { tag, line: bottom });
            b.constrain(|tag| SolverConstraint::Horizontal { tag, line: top });
            b.constrain(|tag| SolverConstraint::Vertical { tag, line: right });
            b.constrain(|tag| SolverConstraint::Vertical { tag, line: left });
            if (i, j) != (n - 1, n - 1) {
                b.constrain(|tag| SolverConstraint::DistanceX {
                    tag,
                    a: PointOf::Start(bottom),
                    b: PointOf::End(bottom),
                    value: w,
                });
                b.constrain(|tag| SolverConstraint::DistanceY {
                    tag,
                    a: PointOf::Start(right),
                    b: PointOf::End(right),
                    value: h,
                });
            }
            let neighbour = match (i, j) {
                (0, 0) => None,
                (_, 0) => Some((rows[i - 1][0], zero, gap)),
                _ => Some((row[j - 1], gap, zero)),
            };
            match neighbour {
                None => b.constrain(|tag| SolverConstraint::Coincident {
                    tag,
                    a: PointOf::Start(bottom),
                    b: PointOf::Point(origin),
                }),
                Some((from, dx, dy)) => {
                    b.constrain(|tag| SolverConstraint::DistanceX {
                        tag,
                        a: PointOf::Start(from),
                        b: PointOf::Start(bottom),
                        value: dx,
                    });
                    b.constrain(|tag| SolverConstraint::DistanceY {
                        tag,
                        a: PointOf::Start(from),
                        b: PointOf::Start(bottom),
                        value: dy,
                    });
                }
            }
            row.push(bottom);
            dragged = right;
        }
        rows.push(row);
    }
    (b.system(), PointOf::End(dragged))
}

/// The point's values in `system`.
fn point_values(system: &SketchSystem, point: PointOf) -> Result<PointValues> {
    let (g, pick) = match point {
        PointOf::Point(g) => (g, 0),
        PointOf::Start(g) => (g, 1),
        PointOf::End(g) => (g, 2),
        PointOf::Center(g) => (g, 3),
    };
    match (system.geometry.get(g.0 as usize), pick) {
        (Some(SketchGeometry::Point(p)), 0) => Ok(*p),
        (Some(SketchGeometry::Line { start, .. }), 1) => Ok(*start),
        (Some(SketchGeometry::Line { end, .. }), 2) => Ok(*end),
        (Some(SketchGeometry::Circle { center, .. }), 3) => Ok(*center),
        _ => Err(format!("the case names {point}, which it doesn't have").into()),
    }
}

/// `system` as solved, as a drag starts from a solved sketch.
fn solved(mut system: SketchSystem) -> Result<SketchSystem> {
    let solved = silico_sketch::solve(&system).map_err(failed)?;
    if solved.status != SolveStatus::Success {
        return Err(format!("the sketch didn't solve: {:?}", solved.status).into());
    }
    system.values = solved.values;
    Ok(system)
}

/// The cursor's path: from the point, 300 steps of 0.05 mm along a circle of radius 10 mm
/// that starts there heading up and to the left.
fn path(from: [f64; 2]) -> Vec<[f64; 2]> {
    let turn = STEP / PATH_RADIUS;
    (1..=FRAMES)
        .map(|k| {
            let t = turn * k as f64;
            [
                from[0] - PATH_RADIUS * (1.0 - libm::cos(t)),
                from[1] + PATH_RADIUS * libm::sin(t),
            ]
        })
        .collect()
}

/// One mode's frames.
struct Frames {
    times: Vec<Duration>,
    solved: usize,
    /// The begin's time, for a drag.
    begin: Option<Duration>,
}

impl Frames {
    fn new(begin: Option<Duration>) -> Self {
        Self {
            times: Vec::with_capacity(FRAMES),
            solved: 0,
            begin,
        }
    }

    /// Records a frame; whether to go on.
    fn record(&mut self, took: Duration, solved: bool, started: Instant) -> bool {
        self.times.push(took);
        self.solved += usize::from(solved);
        self.times.len() < MIN_FRAMES || started.elapsed() < MODE_BUDGET
    }

    fn percentile(&self, p: usize) -> Duration {
        let mut sorted = self.times.clone();
        sorted.sort();
        match sorted.len() {
            0 => Duration::ZERO,
            n => sorted[((n - 1) * p).div_ceil(100).min(n - 1)],
        }
    }

    fn max(&self) -> Duration {
        self.times.iter().copied().max().unwrap_or(Duration::ZERO)
    }
}

/// The system for a full solve with the point at `to`: its coordinates fixed there, the
/// rest from `values`.
fn pinned(system: &SketchSystem, p: PointValues, values: &[f64], to: [f64; 2]) -> SketchSystem {
    let mut s = system.clone();
    s.values.copy_from_slice(values);
    s.values[p.x.0 as usize] = to[0];
    s.values[p.y.0 as usize] = to[1];
    s.unknowns.retain(|&v| v != p.x && v != p.y);
    s
}

fn solve_through(w: &mut Worker, case: &Case, path: &[[f64; 2]]) -> Result<Frames> {
    let p = point_values(&case.system, case.point)?;
    let mut values = case.system.values.clone();
    let mut frames = Frames::new(None);
    let started = Instant::now();
    for &to in path {
        let system = pinned(&case.system, p, &values, to);
        let start = Instant::now();
        let reply = w.call(Generation(1), Op::Solve(system), TIMEOUT)?;
        let took = start.elapsed();
        let Reply::Solved(solved) = reply else {
            return Err(format!("Solve replied {}", reply.kind()).into());
        };
        let ok = solved.status == SolveStatus::Success;
        if ok {
            values = solved.values;
        }
        if !frames.record(took, ok, started) {
            break;
        }
    }
    Ok(frames)
}

fn warm_up(w: &mut Worker, system: &SketchSystem) -> Result<()> {
    w.call(Generation(1), Op::Solve(system.clone()), TIMEOUT)?;
    Ok(())
}

fn solve_here(case: &Case, path: &[[f64; 2]]) -> Result<Frames> {
    let p = point_values(&case.system, case.point)?;
    let mut values = case.system.values.clone();
    let mut frames = Frames::new(None);
    let started = Instant::now();
    for &to in path {
        let system = pinned(&case.system, p, &values, to);
        let start = Instant::now();
        let solved = silico_sketch::solve(&system).map_err(failed)?;
        let took = start.elapsed();
        let ok = solved.status == SolveStatus::Success;
        if ok {
            values = solved.values;
        }
        if !frames.record(took, ok, started) {
            break;
        }
    }
    Ok(frames)
}

fn drag_through(w: &mut Worker, case: &Case, path: &[[f64; 2]]) -> Result<Frames> {
    let begin = Op::DragBegin(DragBegin {
        system: case.system.clone(),
        point: case.point,
    });
    let start = Instant::now();
    let reply = w.call(Generation(1), begin, TIMEOUT)?;
    let began = start.elapsed();
    match reply {
        Reply::DragStarted { conflicting, .. } if conflicting.is_empty() => {}
        other => return Err(format!("DragBegin replied {other:?}").into()),
    }
    let mut frames = Frames::new(Some(began));
    let started = Instant::now();
    for &to in path {
        let start = Instant::now();
        let reply = w.call(Generation(1), Op::DragMove { to }, TIMEOUT)?;
        let took = start.elapsed();
        let Reply::Moved(moved) = reply else {
            return Err(format!("DragMove replied {}", reply.kind()).into());
        };
        if !frames.record(took, moved.status == SolveStatus::Success, started) {
            break;
        }
    }
    w.call(Generation(1), Op::DragEnd, TIMEOUT)?;
    Ok(frames)
}

fn drag_here(case: &Case, path: &[[f64; 2]]) -> Result<Frames> {
    let start = Instant::now();
    let begun = Drag::begin(&DragBegin {
        system: case.system.clone(),
        point: case.point,
    })
    .map_err(failed)?;
    let began = start.elapsed();
    let Some(mut drag) = begun.drag else {
        return Err(format!("the drag found conflicts: {:?}", begun.conflicting).into());
    };
    let mut frames = Frames::new(Some(began));
    let started = Instant::now();
    for &to in path {
        let start = Instant::now();
        let moved = drag.move_to(to).map_err(failed)?;
        let took = start.elapsed();
        if !frames.record(took, moved.status == SolveStatus::Success, started) {
            break;
        }
    }
    Ok(frames)
}

/// A worker's failure, as this benchmark's error.
fn failed(failure: silico_protocol::Failure) -> Box<dyn Error> {
    format!("{failure:?}").into()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() -> Result<()> {
    let mut cases = Vec::new();
    for links in [4, 40, 120, 500] {
        let (system, point) = chain(links);
        cases.push(Case {
            name: format!("chain-{links}"),
            system: solved(system)?,
            point,
        });
    }
    for n in [2, 3, 6, 11] {
        let (system, point) = grid(n);
        cases.push(Case {
            name: format!("grid-{n}x{n}"),
            system: solved(system)?,
            point,
        });
    }

    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "drag latency: {FRAMES} frames of {STEP} mm; frame budget {:.1} ms (60 Hz); times in ms",
        ms(FRAME_BUDGET)
    )?;
    writeln!(
        out,
        "{:<12} {:<10} {:>8} {:>11} {:>9} {:>6} {:>6} {:>9} {:>9} {:>9}",
        "sketch",
        "mode",
        "unknowns",
        "constraints",
        "begin",
        "frames",
        "solved",
        "p50",
        "p99",
        "max"
    )?;
    let mut json = String::new();
    let mut worker = Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch)?;
    for case in &cases {
        let p = point_values(&case.system, case.point)?;
        let from = [
            case.system.values[p.x.0 as usize],
            case.system.values[p.y.0 as usize],
        ];
        let path = path(from);
        // Warmed up, untimed: the worker's first request pays for its pages and symbols.
        warm_up(&mut worker, &case.system)?;
        let modes = [
            ("solve", solve_through(&mut worker, case, &path)?),
            ("drag", drag_through(&mut worker, case, &path)?),
            ("solve-here", solve_here(case, &path)?),
            ("drag-here", drag_here(case, &path)?),
        ];
        let unknowns = case.system.unknowns.len();
        let constraints = case.system.constraints.len();
        for (mode, frames) in &modes {
            let begin = frames
                .begin
                .map_or_else(|| "-".to_owned(), |b| format!("{:.3}", ms(b)));
            writeln!(
                out,
                "{:<12} {:<10} {:>8} {:>11} {:>9} {:>6} {:>6} {:>9.3} {:>9.3} {:>9.3}",
                case.name,
                mode,
                unknowns,
                constraints,
                begin,
                frames.times.len(),
                frames.solved,
                ms(frames.percentile(50)),
                ms(frames.percentile(99)),
                ms(frames.max()),
            )?;
            writeln!(
                json,
                "{{\"sketch\":\"{}\",\"mode\":\"{mode}\",\"unknowns\":{unknowns},\"constraints\":{constraints},\"begin_ms\":{},\"frames\":{},\"solved\":{},\"p50_ms\":{},\"p99_ms\":{},\"max_ms\":{}}}",
                case.name,
                frames
                    .begin
                    .map_or_else(|| "null".to_owned(), |b| ms(b).to_string()),
                frames.times.len(),
                frames.solved,
                ms(frames.percentile(50)),
                ms(frames.percentile(99)),
                ms(frames.max()),
            )?;
        }
        // What IPC adds to a frame, and how much of a full solve is the diagnosis (a drag's
        // begin is the system built and diagnosed, so the rest of a frame's full solve is the
        // solving).
        let p50 = |i: usize| ms(modes[i].1.percentile(50));
        let begin_here = modes[3].1.begin.map_or(0.0, ms);
        writeln!(
            out,
            "{:<12} ipc per frame: solve {:+.3}, drag {:+.3}; full solve p50 {:.3} = diagnosis (begin) {:.3} + solving {:.3}",
            case.name,
            p50(0) - p50(2),
            p50(1) - p50(3),
            p50(2),
            begin_here,
            p50(2) - begin_here,
        )?;
        out.flush()?;
    }
    writeln!(out, "--- json")?;
    write!(out, "{json}")?;
    if let Some(path) = std::env::var_os("SILICO_DRAG_BENCH_OUT") {
        std::fs::write(&path, &json)?;
    }
    Ok(())
}
