#include "silico-sketch/cpp/bridge.h"

#include <algorithm>
#include <cmath>
#include <csignal>
#include <cstddef>
#include <fenv.h>
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

}  // namespace

SolveOutput solve(const SolveInput &input) {
  // Every value in one array, in the core's order, never resized: PlaneGCS orders its
  // unknowns by address, so this makes that order the core's on every platform.
  std::vector<double> values(input.values.begin(), input.values.end());
  auto value = [&](uint32_t i) -> double * {
    if (i >= values.size()) bug("value " + std::to_string(i) + " out of range");
    return &values[i];
  };

  std::vector<Geo> geometry(input.geometry.size());
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
  auto geo = [&](uint32_t i, GeometryKind kind) -> Geo & {
    if (i >= geometry.size() || geometry[i].kind != kind) {
      bug("geometry " + std::to_string(i) + " isn't of the kind the constraint needs");
    }
    return geometry[i];
  };
  auto point_of = [&](Position position, uint32_t i) -> GCS::Point & {
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
  };

  GCS::System system;
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
  if (input.algorithms.empty()) bug("no algorithm to try");

  GCS::VEC_pD unknowns;
  unknowns.reserve(input.unknowns.size());
  for (uint32_t u : input.unknowns) unknowns.push_back(value(u));

  // As FreeCAD's Sketch does: declare, then diagnose once (initSolution).
  system.declareUnknowns(unknowns);
  system.initSolution(GCS::DogLeg);
  SolveOutput out;
  out.dofs = system.dofsNumber();
  GCS::VEC_I tags;
  system.getConflicting(tags);
  for (int t : tags) out.conflicting.push_back(t);
  tags.clear();
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

  // Each algorithm in turn, from the values as given, until one succeeds: FreeCAD's
  // fallback (Sketch::internalSolve), on the one diagnosis.
  int status = GCS::Failed;
  out.succeeded = -1;
  for (size_t i = 0; i < input.algorithms.size(); ++i) {
    status = system.solve(true, algorithm_of(input.algorithms[i]));
    if (status == GCS::Converged && !redundant.empty()) {
      // PlaneGCS checks the constraints it dropped as redundant before writing its
      // solution back (System::solve), so it measures them on the values it started from:
      // an unsolved sketch with a redundancy is never Success. Its own diagnosis writes
      // back first, then checks (identifyConflictingRedundantConstraints); so does this.
      // But Converged also means the solve itself fell short, and PlaneGCS doesn't say
      // which: so every other constraint (the arcs' rules too, tag 0) must be met as the
      // strictest of its algorithms requires of a success (each residual within 1e-10:
      // DogLeg's own test; Levenberg–Marquardt's and BFGS's allow as much or a little
      // more, so this errs towards Converged), and the dropped ones within PlaneGCS's
      // tolerance for them (squared error within `convergence`).
      system.applySolution();
      const auto met = [&](int t, double tolerance) {
        const double e = system.calculateConstraintErrorByTag(t);
        return std::isnan(e) ? false : std::fabs(e) <= tolerance;
      };
      bool all_met = std::all_of(redundant.begin(), redundant.end(), [&](int t) {
        return met(t, std::sqrt(system.convergence));
      });
      for (const ConstraintIn &c : input.constraints) {
        const int t = c.kind == ConstraintKind::ArcRules ? 0 : static_cast<int>(c.tag);
        if (std::find(redundant.begin(), redundant.end(), t) == redundant.end() &&
            !met(t, 1e-10)) {
          all_met = false;
        }
      }
      if (all_met) status = GCS::Success;
    }
    if (status == GCS::Success) {
      out.succeeded = static_cast<int>(i);
      break;
    }
  }
  if (status == GCS::Success) {
    system.applySolution();
  } else {
    system.undoSolution();
  }
  out.status = status_of(status);
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

void debug_enable_fp_traps() {
#if defined(__linux__)
  if (feenableexcept(FE_DIVBYZERO | FE_INVALID | FE_OVERFLOW) == -1) {
    throw std::runtime_error("feenableexcept failed");
  }
#elif defined(__aarch64__)
  // Apple silicon ignores the trap-enable bits: they read back as 0.
  uint64_t fpcr;
  __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
  fpcr |= 0x0700;  // IOE, DZE, OFE
  __asm__ volatile("msr fpcr, %0" : : "r"(fpcr));
#endif
}
#endif

}  // namespace silico_sketch
