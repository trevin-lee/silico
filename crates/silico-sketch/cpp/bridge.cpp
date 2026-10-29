#include "silico-sketch/cpp/bridge.h"

#include <algorithm>
#include <array>
#include <csignal>
#include <cstddef>
#include <fenv.h>
#include <memory>
#include <new>
#include <stdexcept>
#include <string>
#include <vector>

#include <Eigen/Core>

#include "planegcs/GCS.h"
#include "silico-sketch/src/lib.rs.h"

namespace silico_sketch {

namespace {

// One piece of geometry as PlaneGCS takes it: its structs hold pointers into the values.
struct Geo {
  GeometryKind kind = GeometryKind::Point;
  GCS::Point point;
  GCS::Line line;
  GCS::Circle circle;
  GCS::Arc arc;
};

// Every check here repeats one the Rust side made (SketchSystem::check), so a failure is
// a bug in Silico, reported as an exception rather than undefined behavior.
[[noreturn]] void bug(const std::string &what) {
  throw std::logic_error("silico-sketch bridge: " + what);
}

GCS::Algorithm algorithm_of(AlgorithmIn a) {
  switch (a) {
    case AlgorithmIn::DogLeg:
      return GCS::DogLeg;
    case AlgorithmIn::LevenbergMarquardt:
      return GCS::LevenbergMarquardt;
    case AlgorithmIn::Bfgs:
      return GCS::BFGS;
    default:
      break;
  }
  bug("an algorithm Rust doesn't send");
}

Status status_of(int status) {
  switch (status) {
    case GCS::Success:
      return Status::Success;
    case GCS::Converged:
      return Status::Converged;
    case GCS::Failed:
      return Status::Failed;
    default:
      bug("PlaneGCS returned status " + std::to_string(status));
  }
}

// A sketch as PlaneGCS takes it: every value in one array, in the core's order, never
// resized (PlaneGCS orders its unknowns by address, so this makes that order the core's on
// every platform, and its constraints point into it), the geometry made of those values,
// and the system of constraints, its unknowns declared. Built in place and never moved.
struct Sketch {
  std::vector<double> values;
  std::vector<Geo> geometry;
  GCS::System system;
  // The unknowns, in the core's order.
  GCS::VEC_pD unknowns;

  explicit Sketch(const SolveInput &input);
  Sketch(const Sketch &) = delete;
  void add(const ConstraintIn &c, int tag);
  Sketch &operator=(const Sketch &) = delete;

  double *value(uint32_t i) {
    if (i >= values.size()) bug("value " + std::to_string(i) + " out of range");
    return &values[i];
  }

  Geo &geo(uint32_t i, GeometryKind kind) {
    if (i >= geometry.size() || geometry[i].kind != kind) {
      bug("geometry " + std::to_string(i) + " isn't of the kind the constraint needs");
    }
    return geometry[i];
  }

  GCS::Point &point_of(Position position, uint32_t i) {
    if (i >= geometry.size()) bug("geometry " + std::to_string(i) + " out of range");
    Geo &g = geometry[i];
    switch (position) {
      case Position::Point:
        if (g.kind == GeometryKind::Point) return g.point;
        break;
      case Position::Start:
        if (g.kind == GeometryKind::Line) return g.line.p1;
        if (g.kind == GeometryKind::Arc) return g.arc.start;
        break;
      case Position::End:
        if (g.kind == GeometryKind::Line) return g.line.p2;
        if (g.kind == GeometryKind::Arc) return g.arc.end;
        break;
      case Position::Center:
        if (g.kind == GeometryKind::Circle) return g.circle.center;
        if (g.kind == GeometryKind::Arc) return g.arc.center;
        break;
      default:
        break;
    }
    bug("a point geometry " + std::to_string(i) + " doesn't have");
  }

  // A line, circle, or arc, as the curve PlaneGCS's angle at a point takes.
  GCS::Curve &curve(uint32_t i) {
    if (i >= geometry.size()) bug("geometry " + std::to_string(i) + " out of range");
    Geo &g = geometry[i];
    switch (g.kind) {
      case GeometryKind::Line:
        return g.line;
      case GeometryKind::Circle:
        return g.circle;
      case GeometryKind::Arc:
        return g.arc;
      default:
        bug("geometry " + std::to_string(i) + " isn't a curve");
    }
  }

  // A circle, or an arc's circle: its centre and radius.
  GCS::Circle &round(uint32_t i) {
    if (i >= geometry.size()) bug("geometry " + std::to_string(i) + " out of range");
    Geo &g = geometry[i];
    if (g.kind == GeometryKind::Circle) return g.circle;
    if (g.kind == GeometryKind::Arc) return g.arc;
    bug("geometry " + std::to_string(i) + " isn't a circle or an arc");
  }

  bool is_arc(uint32_t i) const {
    return i < geometry.size() && geometry[i].kind == GeometryKind::Arc;
  }

  // A point on a line (extended), a circle, or an arc's whole circle, as FreeCAD's
  // PointOnObject puts it (Sketch::addPointOnObjectConstraint).
  // Returns the index of the row it added, as PlaneGCS's addConstraint calls do.
  int point_on(GCS::Point &p, uint32_t i, int tag) {
    if (i >= geometry.size()) bug("geometry " + std::to_string(i) + " out of range");
    Geo &g = geometry[i];
    switch (g.kind) {
      case GeometryKind::Line:
        return system.addConstraintPointOnLine(p, g.line, tag);
      case GeometryKind::Circle:
        return system.addConstraintPointOnCircle(p, g.circle, tag);
      case GeometryKind::Arc:
        return system.addConstraintPointOnArc(p, g.arc, tag);
      default:
        bug("geometry " + std::to_string(i) + " isn't a curve");
    }
  }

  // The diagnosis, once, as FreeCAD's Sketch makes it (initSolution): the degrees of
  // freedom and the constraints in conflict.
  void diagnose(int &dofs, std::vector<int> &conflicting) {
    system.initSolution(GCS::DogLeg);
    dofs = system.dofsNumber();
    system.getConflicting(conflicting);
  }
};

Sketch::Sketch(const SolveInput &input) : values(input.values.begin(), input.values.end()) {
  geometry.resize(input.geometry.size());
  for (size_t i = 0; i < geometry.size(); ++i) {
    const GeometryIn &in = input.geometry[i];
    const auto &v = in.values;
    auto want = [&](size_t n) {
      if (v.size() != n) bug("geometry " + std::to_string(i) + " has the wrong number of values");
    };
    auto point = [&](size_t at) { return GCS::Point(value(v[at]), value(v[at + 1])); };
    Geo &geo = geometry[i];
    geo.kind = in.kind;
    switch (in.kind) {
      case GeometryKind::Point:
        want(2);
        geo.point = point(0);
        break;
      case GeometryKind::Line:
        want(4);
        geo.line.p1 = point(0);
        geo.line.p2 = point(2);
        break;
      case GeometryKind::Circle:
        want(3);
        geo.circle.center = point(0);
        geo.circle.rad = value(v[2]);
        break;
      case GeometryKind::Arc:
        want(9);
        geo.arc.center = point(0);
        geo.arc.rad = value(v[2]);
        geo.arc.startAngle = value(v[3]);
        geo.arc.endAngle = value(v[4]);
        geo.arc.start = point(5);
        geo.arc.end = point(7);
        break;
      default:
        bug("a geometry kind Rust doesn't send");
    }
  }

  for (const ConstraintIn &c : input.constraints) add(c, static_cast<int>(c.tag));

  unknowns.reserve(input.unknowns.size());
  for (uint32_t u : input.unknowns) unknowns.push_back(value(u));
  system.declareUnknowns(unknowns);
}

// Adds constraint `c`'s rows, tagged `tag`.
void Sketch::add(const ConstraintIn &c, int tag) {
  switch (c.kind) {
    case ConstraintKind::ArcRules:
      system.addConstraintArcRules(geo(c.a, GeometryKind::Arc).arc, 0);
      break;
    case ConstraintKind::Coincident:
      system.addConstraintP2PCoincident(point_of(c.a_position, c.a),
                                        point_of(c.b_position, c.b), tag);
      break;
    case ConstraintKind::Horizontal:
      system.addConstraintHorizontal(geo(c.a, GeometryKind::Line).line, tag);
      break;
    case ConstraintKind::Vertical:
      system.addConstraintVertical(geo(c.a, GeometryKind::Line).line, tag);
      break;
    case ConstraintKind::DistanceX:
      system.addConstraintDifference(point_of(c.a_position, c.a).x,
                                     point_of(c.b_position, c.b).x, value(c.value), tag);
      break;
    case ConstraintKind::DistanceY:
      system.addConstraintDifference(point_of(c.a_position, c.a).y,
                                     point_of(c.b_position, c.b).y, value(c.value), tag);
      break;
    case ConstraintKind::Distance:
      system.addConstraintP2PDistance(point_of(c.a_position, c.a),
                                      point_of(c.b_position, c.b), value(c.value), tag);
      break;
    case ConstraintKind::PointOnLine:
      system.addConstraintPointOnLine(point_of(c.a_position, c.a),
                                      geo(c.b, GeometryKind::Line).line, tag);
      break;
    case ConstraintKind::Parallel:
      system.addConstraintParallel(geo(c.a, GeometryKind::Line).line,
                                   geo(c.b, GeometryKind::Line).line, tag);
      break;
    case ConstraintKind::Perpendicular:
      system.addConstraintPerpendicular(geo(c.a, GeometryKind::Line).line,
                                        geo(c.b, GeometryKind::Line).line, tag);
      break;
    case ConstraintKind::Angle:
      system.addConstraintL2LAngle(geo(c.a, GeometryKind::Line).line,
                                   geo(c.b, GeometryKind::Line).line, value(c.value), tag);
      break;
    case ConstraintKind::EqualLength:
      system.addConstraintEqualLength(geo(c.a, GeometryKind::Line).line,
                                      geo(c.b, GeometryKind::Line).line, tag);
      break;
    case ConstraintKind::Radius:
      // An arc's radius as FreeCAD constrains it (Sketch.cpp, addRadiusConstraint).
      if (c.a < geometry.size() && geometry[c.a].kind == GeometryKind::Arc) {
        system.addConstraintArcRadius(geometry[c.a].arc, value(c.value), tag);
      } else {
        system.addConstraintCircleRadius(geo(c.a, GeometryKind::Circle).circle,
                                         value(c.value), tag);
      }
      break;
    case ConstraintKind::HorizontalPoints:
      system.addConstraintHorizontal(point_of(c.a_position, c.a), point_of(c.b_position, c.b),
                                     tag);
      break;
    case ConstraintKind::VerticalPoints:
      system.addConstraintVertical(point_of(c.a_position, c.a), point_of(c.b_position, c.b),
                                   tag);
      break;
    case ConstraintKind::PointLineDistance:
      system.addConstraintP2LDistance(point_of(c.a_position, c.a),
                                      geo(c.b, GeometryKind::Line).line, value(c.value), tag);
      break;
    case ConstraintKind::PointOnCircle:
      // An arc as FreeCAD puts a point on one: on its whole circle.
      if (is_arc(c.b)) {
        system.addConstraintPointOnArc(point_of(c.a_position, c.a), geometry[c.b].arc, tag);
      } else {
        system.addConstraintPointOnCircle(point_of(c.a_position, c.a),
                                          geo(c.b, GeometryKind::Circle).circle, tag);
      }
      break;
    case ConstraintKind::EqualRadius:
      // As FreeCAD's Sketch::addEqualConstraint: a circle before an arc.
      if (is_arc(c.a) && is_arc(c.b)) {
        system.addConstraintEqualRadius(geometry[c.a].arc, geometry[c.b].arc, tag);
      } else if (is_arc(c.b)) {
        system.addConstraintEqualRadius(geo(c.a, GeometryKind::Circle).circle,
                                        geometry[c.b].arc, tag);
      } else if (is_arc(c.a)) {
        system.addConstraintEqualRadius(geo(c.b, GeometryKind::Circle).circle,
                                        geometry[c.a].arc, tag);
      } else {
        system.addConstraintEqualRadius(geo(c.a, GeometryKind::Circle).circle,
                                        geo(c.b, GeometryKind::Circle).circle, tag);
      }
      break;
    case ConstraintKind::TangentLine:
      if (is_arc(c.b)) {
        system.addConstraintTangent(geo(c.a, GeometryKind::Line).line, geometry[c.b].arc, tag);
      } else {
        system.addConstraintTangent(geo(c.a, GeometryKind::Line).line,
                                    geo(c.b, GeometryKind::Circle).circle, tag);
      }
      break;
    case ConstraintKind::TangentCircles: {
      // FreeCAD's addConstraintTangent of two circles or arcs, its side given rather than
      // read from the geometry.
      GCS::Circle &a = round(c.a);
      GCS::Circle &b = round(c.b);
      system.addConstraintTangentCircumf(a.center, b.center, a.rad, b.rad, c.flag, tag);
      break;
    }
    case ConstraintKind::TangentOn: {
      // FreeCAD's endpoint-to-edge tangency (Sketch::addAngleAtPointConstraint): the point
      // on the curve, then the angle between the curves there, under one tag.
      GCS::Point &p = point_of(c.c_position, c.c);
      point_on(p, c.b, tag);
      system.addConstraintAngleViaPoint(curve(c.a), curve(c.b), p, value(c.value), tag);
      break;
    }
    case ConstraintKind::AngleViaPoint:
      system.addConstraintAngleViaPoint(curve(c.a), curve(c.b), point_of(c.c_position, c.c),
                                        value(c.value), tag);
      break;
    case ConstraintKind::SymmetricLine:
      system.addConstraintP2PSymmetric(point_of(c.a_position, c.a),
                                       point_of(c.b_position, c.b),
                                       geo(c.c, GeometryKind::Line).line, tag);
      break;
    case ConstraintKind::MidpointOnLine: {
      GCS::Line &l = geo(c.c, GeometryKind::Line).line;
      system.addConstraintMidpointOnLine(point_of(c.a_position, c.a),
                                         point_of(c.b_position, c.b), l.p1, l.p2, tag);
      break;
    }
    case ConstraintKind::SymmetricPoint:
      system.addConstraintP2PSymmetric(point_of(c.a_position, c.a),
                                       point_of(c.b_position, c.b),
                                       point_of(c.c_position, c.c), tag);
      break;
    default:
      bug("a constraint kind Rust doesn't send");
  }
}

}  // namespace

// A sketch built and diagnosed once (FreeCAD's Sketch::setUpSketch and the diagnosis in
// initSolution), and the anchors its last algorithm adds.
struct Solver::State {
  Sketch sketch;
  // One per unknown, its starting value: anchored SQP's targets. Sized once, before any
  // anchor points into it, and never resized.
  std::vector<double> anchors;
  bool anchored = false;
  int dofs = 0;
  std::vector<int> conflicting;
  std::vector<int> redundant;
  std::vector<int> partially_redundant;
  std::vector<uint32_t> free;
  bool no_equations = false;

  explicit State(const SolveInput &input) : sketch(input), anchors(input.unknowns.size()) {}
};

Solver::Solver(const SolveInput &input) : state(std::make_unique<State>(input)) {}
Solver::~Solver() = default;

std::unique_ptr<Solver> solver_new(const SolveInput &input) {
  auto solver = std::make_unique<Solver>(input);
  Solver::State &s = *solver->state;
  GCS::System &system = s.sketch.system;
  // As FreeCAD's Sketch does: declare, then diagnose once.
  s.sketch.diagnose(s.dofs, s.conflicting);
  system.getRedundant(s.redundant);
  system.getPartiallyRedundant(s.partially_redundant);
  // PlaneGCS lists a value once for each group of dependent values it's in, and none when
  // there are no equations: no_equations says which (FreeCAD reads it as every unknown
  // free; that's the core's to decide).
  s.no_equations = !input.unknowns.empty() && system.isEmptyDiagnoseMatrix();
  GCS::VEC_pD free;
  system.getDependentParams(free);
  const std::vector<double> &values = s.sketch.values;
  for (double *p : free) {
    const auto at = p - values.data();
    if (at < 0 || static_cast<size_t>(at) >= values.size()) bug("a free value outside the system");
    s.free.push_back(static_cast<uint32_t>(at));
  }
  std::sort(s.free.begin(), s.free.end());
  s.free.erase(std::unique(s.free.begin(), s.free.end()), s.free.end());
  return solver;
}

Diagnosed solver_diagnosis(const Solver &solver) {
  const Solver::State &s = *solver.state;
  Diagnosed out;
  out.dofs = s.dofs;
  for (int t : s.conflicting) out.conflicting.push_back(t);
  for (int t : s.redundant) out.redundant.push_back(t);
  for (int t : s.partially_redundant) out.partially_redundant.push_back(t);
  for (uint32_t v : s.free) out.free.push_back(v);
  out.no_equations = s.no_equations;
  return out;
}

Attempt solver_try(Solver &solver, AlgorithmIn algorithm) {
  Solver::State &s = *solver.state;
  GCS::System &system = s.sketch.system;
  int status = GCS::Failed;
  if (algorithm == AlgorithmIn::AnchoredSqp) {
    // FreeCAD's last resort (Sketch::internalSolve, case 3): every unknown anchored at its
    // starting value by a row tagged -1, which stays out of the diagnosis and puts each
    // part with unknowns under SQP, then solved with the default algorithm.
    if (!s.anchored) {
      GCS::VEC_pD &unknowns = s.sketch.unknowns;
      for (size_t i = 0; i < unknowns.size(); ++i) {
        s.anchors[i] = *unknowns[i];
        system.addConstraintEqual(unknowns[i], &s.anchors[i], GCS::DefaultTemporaryConstraint);
      }
      s.anchored = true;
      system.initSolution();
    }
    status = system.solve(true);
  } else {
    status = system.solve(true, algorithm_of(algorithm));
  }
  Attempt out;
  out.status = status_of(status);
  if (status != GCS::Failed) {
    system.applySolution();
    for (double v : s.sketch.values) out.values.push_back(v);
  }
  // Back to the values the sketch was built with, for the next algorithm.
  system.undoSolution();
  return out;
}

rust::Vec<double> solver_residuals(Solver &solver, rust::Slice<const double> values,
                                   rust::Slice<const uint32_t> tags) {
  Solver::State &s = *solver.state;
  std::vector<double> &own = s.sketch.values;
  if (values.size() != own.size()) bug("residuals asked for on values of another size");
  std::copy(values.begin(), values.end(), own.begin());
  rust::Vec<double> out;
  for (uint32_t tag : tags) {
    out.push_back(s.sketch.system.calculateConstraintErrorByTag(static_cast<int>(tag)));
  }
  return out;
}

// A drag's state (FreeCAD's Sketch::initMove and moveGeometries, absolute moves): the part
// of the sketch the drag moves, diagnosed for its own redundant equations, and the targets
// its pins hold. What to pin, how strongly, and which targets follow the cursor (at what
// offset) is Rust's to say (silico_sketch::Drag::begin).
struct DragSession::State {
  Sketch sketch;
  // Two values per pin: plain values, not unknowns, sized once before any pin points into
  // them and never resized, so the pins' pointers stay good (FreeCAD reserves its
  // MoveParameters for the same reason).
  std::vector<double> targets;
  // For each pin whose target follows the cursor: its index, and its target's base and the
  // point the cursor's offset is measured from (silico_protocol::Follow).
  struct Follower {
    size_t pin;
    double bx, by, fx, fy;
  };
  std::vector<Follower> followers;

  State(const SolveInput &input, size_t pins) : sketch(input), targets(2 * pins) {}
};

DragSession::DragSession(const SolveInput &input, size_t pins)
    : state(std::make_unique<State>(input, pins)) {}
DragSession::~DragSession() = default;

std::unique_ptr<DragSession> drag_begin(const SolveInput &input, rust::Slice<const Pin> pins) {
  if (pins.empty()) bug("a drag with nothing to pin");
  auto session = std::make_unique<DragSession>(input, pins.size());
  DragSession::State &s = *session->state;
  // The part's own diagnosis, which drops its redundant equations. Whether the drag may
  // start (no conflict in the whole sketch, as FreeCAD's) is Rust's to say, before this:
  // a part's rank, found against its own largest entries, can differ from the whole's.
  int dofs = 0;
  std::vector<int> conflicting;
  s.sketch.diagnose(dofs, conflicting);

  GCS::System &system = s.sketch.system;
  for (size_t i = 0; i < pins.size(); ++i) {
    GCS::Point target(&s.targets[2 * i], &s.targets[2 * i + 1]);
    if (pins[i].follows)
      s.followers.push_back({i, pins[i].bx, pins[i].by, pins[i].fx, pins[i].fy});
    // Tagged -1 (GCS::DefaultTemporaryConstraint), so each stays out of the diagnosis and
    // goes to the lower-priority subsystem.
    if (pins[i].position == Position::Geometry) {
      // A target held on a circle or an arc, starting on it at its top, as FreeCAD's
      // (centre plus the radius up): a curve's edge dragged to size it.
      GCS::Circle &c = s.sketch.round(pins[i].geometry);
      s.targets[2 * i] = *c.center.x;
      s.targets[2 * i + 1] = *c.center.y + *c.rad;
      const int row =
          s.sketch.point_on(target, pins[i].geometry, GCS::DefaultTemporaryConstraint);
      if (pins[i].scale != 1.0) system.rescaleConstraint(row, pins[i].scale);
      continue;
    }
    GCS::Point &p = s.sketch.point_of(pins[i].position, pins[i].geometry);
    // Each pin starts where its point is, as FreeCAD's do.
    s.targets[2 * i] = *p.x;
    s.targets[2 * i + 1] = *p.y;
    const int last =
        system.addConstraintP2PCoincident(target, p, GCS::DefaultTemporaryConstraint);
    // Its two rows are the last two added.
    if (pins[i].scale != 1.0) {
      system.rescaleConstraint(last - 1, pins[i].scale);
      system.rescaleConstraint(last, pins[i].scale);
    }
  }
  // Keeps the diagnosis (rows tagged below 0 don't touch it), splits the system again with
  // the pins, and makes these values the ones every move starts from.
  system.initSolution();
  return session;
}

Diagnosis diagnose(const SolveInput &input) {
  Sketch sketch(input);
  Diagnosis out;
  std::vector<int> conflicting;
  sketch.diagnose(out.dofs, conflicting);
  for (int t : conflicting) out.conflicting.push_back(t);
  return out;
}

MoveOutput drag_move(DragSession &session, double x, double y) {
  DragSession::State &s = *session.state;
  GCS::System &system = s.sketch.system;
  for (const auto &f : s.followers) {
    // In FreeCAD's order: the base plus the cursor's offset, so a cursor back where it
    // started gives the base exactly.
    s.targets[2 * f.pin] = f.bx + (x - f.fx);
    s.targets[2 * f.pin + 1] = f.by + (y - f.fy);
  }
  // DogLeg alone, with no fallback, as FreeCAD drags; a part with a pin is solved by SQP.
  // Each solve starts from the values at the drag's start (System::solve resets to them).
  const int status = system.solve(true, GCS::DogLeg);
  MoveOutput out;
  out.status = status_of(status);
  if (status == GCS::Success) {
    system.applySolution();
    for (double v : s.sketch.values) out.values.push_back(v);
  } else {
    system.undoSolution();
  }
  return out;
}

uint32_t fp_trap_mask() {
#if defined(__linux__)
  // glibc reports the enabled traps; fegetexcept computes nothing, so it's exact.
  return static_cast<uint32_t>(fegetexcept());
#elif defined(__aarch64__)
  // The trap-enable bits of FPCR: IOE, DZE, OFE, UFE, IXE (8-12) and IDE (15). Read
  // directly, since macOS has no fegetexcept.
  uint64_t fpcr;
  __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
  return static_cast<uint32_t>(fpcr & 0x9f00);
#else
#error "fp_trap_mask: unsupported platform"
#endif
}

void disable_fp_traps() {
#if defined(__linux__)
  if (fedisableexcept(FE_ALL_EXCEPT) == -1) throw std::runtime_error("fedisableexcept failed");
#elif defined(__aarch64__)
  uint64_t fpcr;
  __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
  fpcr &= ~static_cast<uint64_t>(0x9f00);
  __asm__ volatile("msr fpcr, %0" : : "r"(fpcr));
#else
#error "disable_fp_traps: unsupported platform"
#endif
}

void fix_eigen_cache_sizes() {
  // Eigen's own defaults where it can't ask the CPU (ARM): on x86 it asks, and blocks
  // large products differently.
  Eigen::setCpuCacheSizes(16 * 1024, 512 * 1024, 512 * 1024);
}

#ifdef SILICO_TEST_OPS
void debug_throw_runtime_error(rust::Str message) {
  throw std::runtime_error(std::string(message));
}

void debug_raise_sigsegv() {
  // As the kernel bridge does: restore the default action first, since Rust's handler
  // returns from a raised signal.
  std::signal(SIGSEGV, SIG_DFL);
  std::raise(SIGSEGV);
}

void debug_throw_bad_alloc() { throw std::bad_alloc(); }

rust::Vec<uint64_t> debug_eigen_cache_sizes() {
  rust::Vec<uint64_t> out;
  out.push_back(static_cast<uint64_t>(Eigen::l1CacheSize()));
  out.push_back(static_cast<uint64_t>(Eigen::l2CacheSize()));
  out.push_back(static_cast<uint64_t>(Eigen::l3CacheSize()));
  return out;
}

void debug_enable_fp_traps() {
#if defined(__linux__)
  if (feenableexcept(FE_DIVBYZERO | FE_INVALID | FE_OVERFLOW) == -1) {
    throw std::runtime_error("feenableexcept failed");
  }
#elif defined(__aarch64__)
  // Some Apple silicon ignores the trap-enable bits (they read back as 0); some keeps them.
  uint64_t fpcr;
  __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
  fpcr |= 0x0700;  // IOE, DZE, OFE
  __asm__ volatile("msr fpcr, %0" : : "r"(fpcr));
#endif
}
#endif

}  // namespace silico_sketch
