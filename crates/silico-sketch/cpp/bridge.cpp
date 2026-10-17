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

  explicit Sketch(const SolveInput &input);
  Sketch(const Sketch &) = delete;
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

  // The diagnosis, once, as FreeCAD's Sketch makes it (initSolution): the degrees of
  // freedom and the constraints in conflict.
  void diagnose(int &dofs, rust::Vec<int32_t> &conflicting) {
    system.initSolution(GCS::DogLeg);
    dofs = system.dofsNumber();
    GCS::VEC_I tags;
    system.getConflicting(tags);
    for (int t : tags) conflicting.push_back(t);
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

  for (const ConstraintIn &c : input.constraints) {
    const int tag = static_cast<int>(c.tag);
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
      default:
        bug("a constraint kind Rust doesn't send");
    }
  }

  GCS::VEC_pD unknowns;
  unknowns.reserve(input.unknowns.size());
  for (uint32_t u : input.unknowns) unknowns.push_back(value(u));
  system.declareUnknowns(unknowns);
}

}  // namespace

SolveOutput solve(const SolveInput &input) {
  if (input.algorithms.empty()) bug("no algorithm to try");
  Sketch sketch(input);
  std::vector<double> &values = sketch.values;
  GCS::System &system = sketch.system;

  // As FreeCAD's Sketch does: declare, then diagnose once.
  SolveOutput out;
  sketch.diagnose(out.dofs, out.conflicting);
  GCS::VEC_I tags;
  system.getRedundant(tags);
  for (int t : tags) out.redundant.push_back(t);
  const std::vector<int> redundant = tags;
  tags.clear();
  system.getPartiallyRedundant(tags);
  for (int t : tags) out.partially_redundant.push_back(t);
  // PlaneGCS lists a value once for each group of dependent values it's in, and none when
  // there are no equations: no_equations says which (FreeCAD reads it as every unknown
  // free; that's the core's to decide).
  out.no_equations = !input.unknowns.empty() && system.isEmptyDiagnoseMatrix();
  GCS::VEC_pD free;
  system.getDependentParams(free);
  std::vector<uint32_t> free_values;
  for (double *p : free) {
    const auto at = p - values.data();
    if (at < 0 || static_cast<size_t>(at) >= values.size()) bug("a free value outside the system");
    free_values.push_back(static_cast<uint32_t>(at));
  }
  std::sort(free_values.begin(), free_values.end());
  free_values.erase(std::unique(free_values.begin(), free_values.end()), free_values.end());
  for (uint32_t v : free_values) out.free.push_back(v);

  // Each algorithm in turn, from the values as given, until one solves: FreeCAD's fallback
  // (Sketch::internalSolve), on the one diagnosis. (Its last step, SQP on a system anchored
  // at the given values, comes with M3.3.)
  //
  // With equations dropped as redundant, System::solve checks the dropped ones on the
  // values it started from, before its solution is written back, and turns Success into
  // Converged if they miss there; so whether it says Success depends on where the sketch
  // was drawn, not on the solution. A solve with equations dropped is reported as solved
  // except them either way: everything it kept is met, and the dropped ones' residuals on
  // the solution say whether they agree, the core's to judge (docs/roadmap/m3-sketcher.md,
  // decision 5). DogLeg and Levenberg–Marquardt end only in Success or Failed (GCS.cpp,
  // solve_DL and solve_LM), so their Converged is that check. That holds while every
  // constraint is driving with a tag of 0 or more: then there's no auxiliary subsystem,
  // whose solver (SQP) ends Converged of its own accord. BFGS can too, so its Converged
  // stays one, unsolved, as FreeCAD reads it; with equations dropped, it stays read on the
  // drawing.
  const bool dropped = !redundant.empty() || !out.partially_redundant.empty();
  Status reported = Status::Failed;
  out.succeeded = -1;
  for (size_t i = 0; i < input.algorithms.size(); ++i) {
    const GCS::Algorithm algorithm = algorithm_of(input.algorithms[i]);
    const int status = system.solve(true, algorithm);
    const bool solved =
        status == GCS::Success || (status == GCS::Converged && algorithm != GCS::BFGS && dropped);
    if (!solved) {
      reported = status_of(status);
      continue;
    }
    reported = dropped ? Status::SolvedExceptRedundant : Status::Success;
    out.succeeded = static_cast<int>(i);
    break;
  }
  if (out.succeeded >= 0) {
    system.applySolution();
  } else {
    system.undoSolution();
  }
  out.status = reported;
  for (double v : values) out.values.push_back(v);
  // Every tagged constraint's residual on those values: PlaneGCS solves only the parts
  // with unknowns, so this is how a constraint among fixed values alone shows whether it
  // holds.
  std::vector<int> tagged;
  for (const ConstraintIn &c : input.constraints) {
    if (c.kind != ConstraintKind::ArcRules) tagged.push_back(static_cast<int>(c.tag));
  }
  std::sort(tagged.begin(), tagged.end());
  for (int tag : tagged) {
    out.residual_tags.push_back(static_cast<uint32_t>(tag));
    out.residuals.push_back(system.calculateConstraintErrorByTag(tag));
  }
  return out;
}

// A drag's state (FreeCAD's Sketch::initMove and moveGeometries, absolute moves): the
// sketch, diagnosed once, and the targets its pins hold points to. Which points to pin, and
// how strongly, is Rust's to say (silico_sketch::Drag::begin); the first pin's target is
// the cursor.
struct DragSession::State {
  Sketch sketch;
  // Two values per pin: plain values, not unknowns, sized once before any pin points into
  // them and never resized, so the pins' pointers stay good (FreeCAD reserves its
  // MoveParameters for the same reason).
  std::vector<double> targets;

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
  rust::Vec<int32_t> conflicting;
  s.sketch.diagnose(dofs, conflicting);

  GCS::System &system = s.sketch.system;
  for (size_t i = 0; i < pins.size(); ++i) {
    GCS::Point &p = s.sketch.point_of(pins[i].position, pins[i].geometry);
    // Each pin starts where its point is, as FreeCAD's do; tagged -1
    // (GCS::DefaultTemporaryConstraint), so it stays out of the diagnosis and goes to the
    // lower-priority subsystem.
    s.targets[2 * i] = *p.x;
    s.targets[2 * i + 1] = *p.y;
    GCS::Point target(&s.targets[2 * i], &s.targets[2 * i + 1]);
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

DragDiagnosis diagnose(const SolveInput &input) {
  Sketch sketch(input);
  DragDiagnosis out;
  sketch.diagnose(out.dofs, out.conflicting);
  return out;
}

MoveOutput drag_move(DragSession &session, double x, double y) {
  DragSession::State &s = *session.state;
  GCS::System &system = s.sketch.system;
  s.targets[0] = x;
  s.targets[1] = y;
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
