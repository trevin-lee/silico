// The STEP worker's C++ (docs/ports/step-export.md): bodies written as one STEP file by OCCT's
// XCAF writer. Its own file and bridge, linked into silico-step-worker alone, so the kernel
// worker carries none of STEP's toolkits (nor the windowing frameworks TKService loads).
//
// Every function here may throw: bridge.h's trycatch turns it into a Rust Err.
#pragma once

#include "silico-kernel/cpp/bridge.h"

#include <TopoDS_Shape.hxx>

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#include "rust/cxx.h"

namespace silico {

enum class StepKind : uint8_t;

// A STEP file written: its bytes; each part's faces left out of it as bounding no material
// (INTERNAL and EXTERNAL, which OCCT's writer drops), numbered as TopExp::MapShapes numbers
// them, their counts by part; each entry of the writer's check list with a failure, of
// anything but a face left out: the part its shape is of (UINT32_MAX if none), its kind (a
// face, edge, or vertex of the part, or the part itself, or none), its index among them, and
// OCCT's messages, one to a line; what it warned of, as it corrected it; and, for the caller
// to compare, the faces the parts have that bound material against the face surfaces the file
// holds, and those faces' bounds (their wires) against the face bounds it holds.
class StepOut {
public:
  rust::Slice<const uint8_t> bytes() const { return {bytes_.data(), bytes_.size()}; }
  rust::Slice<const uint32_t> left_out_counts() const {
    return {left_out_counts_.data(), left_out_counts_.size()};
  }
  rust::Slice<const uint32_t> left_out() const { return {left_out_.data(), left_out_.size()}; }
  rust::Slice<const uint32_t> unwritten_parts() const {
    return {unwritten_parts_.data(), unwritten_parts_.size()};
  }
  rust::Slice<const StepKind> unwritten_kinds() const {
    return {unwritten_kinds_.data(), unwritten_kinds_.size()};
  }
  rust::Slice<const uint32_t> unwritten_indices() const {
    return {unwritten_indices_.data(), unwritten_indices_.size()};
  }
  rust::Vec<rust::String> unwritten_messages() const;
  rust::Vec<rust::String> warnings() const;
  uint64_t faces_expected() const { return faces_expected_; }
  uint64_t faces_written() const { return faces_written_; }
  uint64_t bounds_expected() const { return bounds_expected_; }
  uint64_t bounds_written() const { return bounds_written_; }

  std::vector<uint8_t> bytes_;
  std::vector<uint32_t> left_out_counts_;
  std::vector<uint32_t> left_out_;
  std::vector<uint32_t> unwritten_parts_;
  std::vector<StepKind> unwritten_kinds_;
  std::vector<uint32_t> unwritten_indices_;
  std::vector<std::string> unwritten_messages_;
  std::vector<std::string> warnings_;
  uint64_t faces_expected_ = 0;
  uint64_t faces_written_ = 0;
  uint64_t bounds_expected_ = 0;
  uint64_t bounds_written_ = 0;
};

// The parts of a STEP file, added in the file's order (each from its binary B-rep, the
// checksum Silico appends already checked and taken off), then written as one: each a product
// whose id and name are given, under an assembly named `assembly` if there are two or more; by
// STEPCAFControl_Writer from an XCAF document of its own, every setting given as
// DESTEP_Parameters (never a process-wide static), and every header field set from the
// arguments or Silico's constants, so the bytes are the same every run. The names, the
// assembly's, and the file name are the worker's tokens for its strings, which it splices
// into the bytes returned (silico-step-worker.rs, `Strings`); the ids, feature IDs, are set
// as they are.
class StepWriter {
public:
  void add_part(rust::Slice<const uint8_t> brep, rust::Str id, rust::Str name);
  std::unique_ptr<StepOut> write(rust::Str assembly, rust::Str file_name) const;

private:
  std::vector<TopoDS_Shape> shapes_;
  std::vector<std::string> ids_;
  std::vector<std::string> names_;
};

std::unique_ptr<StepWriter> step_writer();

} // namespace silico
