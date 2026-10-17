#include "silico-sketch/cpp/bridge.h"

#include <algorithm>
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

// The encodings src/lib.rs writes: see `encode` there.
constexpr size_t GEOMETRY_STRIDE = 10;
constexpr size_t CONSTRAINT_STRIDE = 8;

enum GeometryKind : uint32_t { POINT = 0, LINE = 1, CIRCLE = 2, ARC = 3 };
enum Position : uint32_t { NONE = 0, POINT_ITSELF = 1, START = 2, END = 3, CENTER = 4 };
enum ConstraintKind : uint32_t {
  ARC_RULES = 0,
  COINCIDENT = 1,
  HORIZONTAL = 2,
  VERTICAL = 3,
  DISTANCE_X = 4,
  DISTANCE_Y = 5,
  DISTANCE = 6,
  POINT_ON_LINE = 7,
  PARALLEL = 8,
  PERPENDICULAR = 9,
  ANGLE = 10,
  EQUAL_LENGTH = 11,
  RADIUS = 12,
};

// One piece of geometry as PlaneGCS takes it: its structs hold pointers into the values.
struct Geo {
  uint32_t kind = POINT;
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

}  // namespace

SolveOutput solve(const SolveInput &input) {
  // Every value in one array, in the core's order, never resized: PlaneGCS orders its
  // unknowns by address, so this makes that order the core's on every platform.
  std::vector<double> values(input.values.begin(), input.values.end());
  auto value = [&](uint32_t i) -> double * {
    if (i >= values.size()) bug("value " + std::to_string(i) + " out of range");
    return &values[i];
  };
  auto point = [&](const uint32_t *v) { return GCS::Point(value(v[0]), value(v[1])); };

  if (input.geometry.size() % GEOMETRY_STRIDE != 0) bug("geometry encoding");
  std::vector<Geo> geometry(input.geometry.size() / GEOMETRY_STRIDE);
  for (size_t i = 0; i < geometry.size(); ++i) {
    const uint32_t *g = input.geometry.data() + i * GEOMETRY_STRIDE;
    const uint32_t *v = g + 1;
    Geo &geo = geometry[i];
    geo.kind = g[0];
    switch (g[0]) {
      case POINT:
        geo.point = point(v);
        break;
      case LINE:
        geo.line.p1 = point(v);
        geo.line.p2 = point(v + 2);
        break;
      case CIRCLE:
        geo.circle.center = point(v);
        geo.circle.rad = value(v[2]);
        break;
      case ARC:
        geo.arc.center = point(v);
        geo.arc.rad = value(v[2]);
        geo.arc.startAngle = value(v[3]);
        geo.arc.endAngle = value(v[4]);
        geo.arc.start = point(v + 5);
        geo.arc.end = point(v + 7);
        break;
      default:
        bug("geometry kind " + std::to_string(g[0]));
    }
  }
  auto geo = [&](uint32_t i, uint32_t kind) -> Geo & {
    if (i >= geometry.size() || geometry[i].kind != kind) {
      bug("geometry " + std::to_string(i) + " isn't of kind " + std::to_string(kind));
    }
    return geometry[i];
  };
  auto point_of = [&](uint32_t position, uint32_t i) -> GCS::Point & {
    if (i >= geometry.size()) bug("geometry " + std::to_string(i) + " out of range");
    Geo &g = geometry[i];
    switch (position) {
      case POINT_ITSELF:
        if (g.kind == POINT) return g.point;
        break;
      case START:
        if (g.kind == LINE) return g.line.p1;
        if (g.kind == ARC) return g.arc.start;
        break;
      case END:
        if (g.kind == LINE) return g.line.p2;
        if (g.kind == ARC) return g.arc.end;
        break;
      case CENTER:
        if (g.kind == CIRCLE) return g.circle.center;
        if (g.kind == ARC) return g.arc.center;
        break;
      default:
        break;
    }
    bug("position " + std::to_string(position) + " of geometry " + std::to_string(i));
  };

  GCS::System system;
  if (input.constraints.size() % CONSTRAINT_STRIDE != 0) bug("constraint encoding");
  for (size_t i = 0; i < input.constraints.size(); i += CONSTRAINT_STRIDE) {
    const uint32_t *c = input.constraints.data() + i;
    const int tag = static_cast<int>(c[1]);
    const uint32_t a_pos = c[2], a = c[3], b_pos = c[4], b = c[5];
    switch (c[0]) {
      case ARC_RULES:
        system.addConstraintArcRules(geo(a, ARC).arc, 0);
        break;
      case COINCIDENT:
        system.addConstraintP2PCoincident(point_of(a_pos, a), point_of(b_pos, b), tag);
        break;
      case HORIZONTAL:
        system.addConstraintHorizontal(geo(a, LINE).line, tag);
        break;
      case VERTICAL:
        system.addConstraintVertical(geo(a, LINE).line, tag);
        break;
      case DISTANCE_X:
        system.addConstraintDifference(point_of(a_pos, a).x, point_of(b_pos, b).x, value(c[6]),
                                       tag);
        break;
      case DISTANCE_Y:
        system.addConstraintDifference(point_of(a_pos, a).y, point_of(b_pos, b).y, value(c[6]),
                                       tag);
        break;
      case DISTANCE:
        system.addConstraintP2PDistance(point_of(a_pos, a), point_of(b_pos, b), value(c[6]), tag);
        break;
      case POINT_ON_LINE:
        system.addConstraintPointOnLine(point_of(a_pos, a), geo(b, LINE).line, tag);
        break;
      case PARALLEL:
        system.addConstraintParallel(geo(a, LINE).line, geo(b, LINE).line, tag);
        break;
      case PERPENDICULAR:
        system.addConstraintPerpendicular(geo(a, LINE).line, geo(b, LINE).line, tag);
        break;
      case ANGLE:
        system.addConstraintL2LAngle(geo(a, LINE).line, geo(b, LINE).line, value(c[6]), tag);
        break;
      case EQUAL_LENGTH:
        system.addConstraintEqualLength(geo(a, LINE).line, geo(b, LINE).line, tag);
        break;
      case RADIUS:
        if (a < geometry.size() && geometry[a].kind == ARC) {
          system.addConstraintArcRadius(geometry[a].arc, value(c[6]), tag);
        } else {
          system.addConstraintCircleRadius(geo(a, CIRCLE).circle, value(c[6]), tag);
        }
        break;
      default:
        bug("constraint kind " + std::to_string(c[0]));
    }
  }

  GCS::VEC_pD unknowns;
  unknowns.reserve(input.unknowns.size());
  for (uint32_t u : input.unknowns) unknowns.push_back(value(u));
  GCS::Algorithm algorithm;
  switch (input.algorithm) {
    case 0:
      algorithm = GCS::DogLeg;
      break;
    case 1:
      algorithm = GCS::LevenbergMarquardt;
      break;
    case 2:
      algorithm = GCS::BFGS;
      break;
    default:
      bug("algorithm " + std::to_string(input.algorithm));
  }

  // As FreeCAD's Sketch does: declare, diagnose (initSolution), then solve from the
  // values as given.
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
  tags.clear();
  system.getPartiallyRedundant(tags);
  for (int t : tags) out.partially_redundant.push_back(t);
  GCS::VEC_pD free;
  system.getDependentParams(free);
  std::vector<uint32_t> free_values;
  for (double *p : free) {
    const auto at = p - values.data();
    if (at < 0 || static_cast<size_t>(at) >= values.size()) bug("a free value outside the system");
    free_values.push_back(static_cast<uint32_t>(at));
  }
  std::sort(free_values.begin(), free_values.end());
  for (uint32_t v : free_values) out.free.push_back(v);

  const int status = system.solve(true, algorithm);
  if (status == GCS::Success) {
    system.applySolution();
  } else {
    system.undoSolution();
  }
  out.status = status;
  for (double v : values) out.values.push_back(v);
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
#endif

}  // namespace silico_sketch
