#include "silico-kernel/cpp/bridge.h"
#include "silico-kernel/src/lib.rs.h"

#include <BRepGProp.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <GProp_GProps.hxx>
#include <gp_Pnt.hxx>

#include <stdexcept>

namespace silico {

std::unique_ptr<Shape> make_box(double dx, double dy, double dz) {
  // Throws Standard_DomainError for a size at or below Precision::Confusion().
  BRepPrimAPI_MakeBox box(dx, dy, dz);
  return std::make_unique<Shape>(Shape{box.Shape()});
}

MassProperties mass_properties(const Shape &shape) {
  GProp_GProps props;
  BRepGProp::VolumeProperties(shape.shape, props);
  gp_Pnt c = props.CentreOfMass();
  return MassProperties{props.Mass(), Point3{c.X(), c.Y(), c.Z()}};
}

#ifdef SILICO_TEST_OPS
void debug_throw_standard_failure(rust::Str message) {
  throw Standard_Failure(std::string(message).c_str());
}

void debug_throw_std_runtime_error(rust::Str message) {
  throw std::runtime_error(std::string(message));
}

void debug_throw_non_exception() { throw 42; }
#endif

} // namespace silico
