// The STEP worker's C++ (docs/ports/step-export.md): see step.h.

#include "silico-kernel/cpp/step.h"
#include "silico-kernel/src/bin/silico-step-worker.rs.h"

#include <APIHeaderSection_MakeHeader.hxx>
#include <DESTEP_Parameters.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <Interface_Check.hxx>
#include <Interface_CheckIterator.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_Printer.hxx>
#include <STEPCAFControl_Writer.hxx>
#include <STEPControl_Writer.hxx>
#include <Standard_Version.hxx>
#include <StepBasic_Product.hxx>
#include <StepBasic_ProductDefinition.hxx>
#include <StepBasic_ProductDefinitionFormation.hxx>
#include <StepData_StepModel.hxx>
#include <StepRepr_CharacterizedDefinition.hxx>
#include <StepRepr_NextAssemblyUsageOccurrence.hxx>
#include <StepRepr_PropertyDefinition.hxx>
#include <StepShape_FaceSurface.hxx>
#include <StepShape_ShapeDefinitionRepresentation.hxx>
#include <TCollection_ExtendedString.hxx>
#include <TCollection_HAsciiString.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <Transfer_FinderProcess.hxx>
#include <TransferBRep.hxx>
#include <TransferBRep_ShapeMapper.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <XSControl_TransferWriter.hxx>
#include <XSControl_WorkSession.hxx>
#include <BRep_Builder.hxx>
#include <BinTools.hxx>
#include <StdFail_NotDone.hxx>
#include <Standard_DomainError.hxx>
#include <StepShape_FaceBound.hxx>
#include <StepShape_FaceSurface.hxx>
#include <TopAbs.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopLoc_Location.hxx>
#include <TopoDS_Compound.hxx>
#include <UnitsMethods_LengthUnit.hxx>
#include <XCAFDoc_DocumentTool.hxx>

#include <algorithm>
#include <cstdint>
#include <istream>
#include <map>
#include <sstream>
#include <streambuf>
#include <string>
#include <vector>

namespace silico {

namespace {
// Reads straight from the caller's bytes, as the kernel bridge does. Seekable, because
// BinTools moves around in the stream as it reads.
class ByteStreamBuf : public std::streambuf {
public:
  ByteStreamBuf(const char *data, size_t size) {
    char *begin = const_cast<char *>(data);
    setg(begin, begin, begin + size);
  }

protected:
  pos_type seekoff(off_type off, std::ios_base::seekdir dir,
                   std::ios_base::openmode which) override {
    if (!(which & std::ios_base::in)) {
      return pos_type(off_type(-1));
    }
    char *base = dir == std::ios_base::beg   ? eback()
                 : dir == std::ios_base::cur ? gptr()
                                             : egptr();
    const off_type target = (base - eback()) + off;
    if (target < 0 || target > egptr() - eback()) {
      return pos_type(off_type(-1));
    }
    setg(eback(), eback() + target, egptr());
    return pos_type(target);
  }

  pos_type seekpos(pos_type pos, std::ios_base::openmode which) override {
    return seekoff(off_type(pos), std::ios_base::beg, which);
  }
};
} // namespace
// OCCT's messages, as Rust strings, any byte that isn't UTF-8 replaced: a message is never a
// reason for the worker to stop.
rust::Vec<rust::String> StepOut::unwritten_messages() const {
  rust::Vec<rust::String> messages;
  for (const std::string &m : unwritten_messages_) {
    messages.push_back(rust::String::lossy(m));
  }
  return messages;
}

rust::Vec<rust::String> StepOut::warnings() const {
  rust::Vec<rust::String> warnings;
  for (const std::string &w : warnings_) {
    warnings.push_back(rust::String::lossy(w));
  }
  return warnings;
}

std::unique_ptr<StepWriter> step_writer() { return std::make_unique<StepWriter>(); }

void StepWriter::add_part(rust::Slice<const uint8_t> brep, rust::Str id, rust::Str name) {
  ByteStreamBuf buffer(reinterpret_cast<const char *>(brep.data()), brep.size());
  std::istream in(&buffer);
  TopoDS_Shape shape;
  BinTools::Read(shape, in);
  if (shape.IsNull()) {
    throw Standard_Failure("a part isn't a B-rep");
  }
  shapes_.push_back(shape);
  ids_.emplace_back(id.data(), id.size());
  names_.emplace_back(name.data(), name.size());
}

namespace {

// The header's fields Silico sets, every one, after the transfer (which writes OCCT's own,
// the transfer's time among them): fixed but for the document's file name.
void write_header(const Handle(StepData_StepModel) &model, const std::string &file_name) {
  APIHeaderSection_MakeHeader header(model);
  header.SetName(new TCollection_HAsciiString(file_name.c_str()));
  header.SetTimeStamp(new TCollection_HAsciiString("1970-01-01T00:00:00"));
  header.SetAuthorValue(1, new TCollection_HAsciiString(""));
  header.SetOrganizationValue(1, new TCollection_HAsciiString(""));
  header.SetPreprocessorVersion(
      new TCollection_HAsciiString("Open CASCADE STEP processor " OCC_VERSION_STRING));
  header.SetOriginatingSystem(new TCollection_HAsciiString("Silico"));
  header.SetAuthorisation(new TCollection_HAsciiString(""));
  header.SetDescriptionValue(1, new TCollection_HAsciiString(""));
}

// A label's name, from a string already encoded (ASCII).
void set_name(const TDF_Label &label, const std::string &name) {
  TDataStd_Name::Set(label, TCollection_ExtendedString(name.c_str()));
}

// What OCCT is given in place of each string Silico writes (docs/ports/step-export.md,
// *Strings*): a token no shape of OCCT's names anything, short enough that it never wraps.
// OCCT breaks a string past 72 columns, inside an encoded run too, and its reader decodes
// before it joins the lines, so OCCT writes none of Silico's strings itself: each is spliced
// in where its token is, once the file is written.
std::string placeholder(const char *what, size_t index) {
  return std::string("SILICO-") + what + "-" + std::to_string(index);
}

// `text` with each `'TOKEN'` of `strings` (every key's quoted form, starting `'SILICO-`) made
// `'VALUE'`: refused if a key isn't found, or a value holds a `'` (Silico's encoding has
// none). One pass, so it's as long as the file, however many there are.
std::string spliced(const std::string &text, const std::map<std::string, std::string> &strings) {
  for (const auto &kv : strings) {
    if (kv.second.find('\'') != std::string::npos) {
      throw Standard_DomainError("a STEP string holds a quote: it isn't encoded");
    }
  }
  std::string out;
  out.reserve(text.size());
  std::map<std::string, size_t> found;
  const std::string open = "'SILICO-";
  size_t at = 0;
  while (true) {
    const size_t next = text.find(open, at);
    if (next == std::string::npos) {
      out.append(text, at, std::string::npos);
      break;
    }
    const size_t close = text.find('\'', next + 1);
    if (close == std::string::npos) {
      throw StdFail_NotDone("OCCT's STEP writer left a placeholder unclosed");
    }
    const std::string token = text.substr(next + 1, close - next - 1);
    const auto value = strings.find(token);
    if (value == strings.end()) {
      throw StdFail_NotDone("OCCT's STEP writer wrote a placeholder Silico didn't give");
    }
    out.append(text, at, next - at);
    out += '\'';
    out += value->second;
    out += '\'';
    ++found[token];
    at = close + 1;
  }
  for (const auto &kv : strings) {
    if (found[kv.first] == 0) {
      throw StdFail_NotDone(("OCCT's STEP writer wrote no " + kv.first).c_str());
    }
  }
  return out;
}

} // namespace

std::unique_ptr<StepOut> StepWriter::write(rust::Str assembly, rust::Str file_name) const {
  if (shapes_.empty()) {
    throw Standard_DomainError("a STEP export needs at least one part");
  }
  // The transfer reports its statistics through the default messenger, which prints them:
  // nothing of a worker's goes to its stdout.
  static const bool silenced = [] {
    Message::DefaultMessenger()->RemovePrinters(STANDARD_TYPE(Message_Printer));
    return true;
  }();
  (void)silenced;
  auto out = std::make_unique<StepOut>();
  // Each part's faces, edges, and vertices, numbered as TopExp::MapShapes numbers them (as
  // the element map does); its faces that bound no material, which the writer drops.
  const size_t parts = shapes_.size();
  std::vector<TopTools_IndexedMapOfShape> faces(parts), edges(parts), vertices(parts);
  for (size_t i = 0; i < parts; ++i) {
    TopExp::MapShapes(shapes_[i], TopAbs_FACE, faces[i]);
    TopExp::MapShapes(shapes_[i], TopAbs_EDGE, edges[i]);
    TopExp::MapShapes(shapes_[i], TopAbs_VERTEX, vertices[i]);
    uint32_t count = 0;
    for (int f = 1; f <= faces[i].Extent(); ++f) {
      const TopAbs_Orientation o = faces[i](f).Orientation();
      if (o == TopAbs_INTERNAL || o == TopAbs_EXTERNAL) {
        out->left_out_.push_back(static_cast<uint32_t>(f));
        ++count;
      }
    }
    out->left_out_counts_.push_back(count);
  }

  // Each part as XCAF takes it: a located shape it would make a reference to a prototype
  // of, two products for one body (the outer named, the inner OCCT's "SOLID"), so it's
  // given in a compound of its own, which has none.
  std::vector<TopoDS_Shape> given(parts);
  for (size_t i = 0; i < parts; ++i) {
    if (shapes_[i].Location().IsIdentity()) {
      given[i] = shapes_[i];
    } else {
      TopoDS_Compound compound;
      BRep_Builder builder;
      builder.MakeCompound(compound);
      builder.Add(compound, shapes_[i]);
      given[i] = compound;
    }
  }
  // Each string the file holds, by the placeholder OCCT is given for it.
  std::map<std::string, std::string> strings;
  std::vector<std::string> names(parts);
  for (size_t i = 0; i < parts; ++i) {
    names[i] = placeholder("NAME", i);
    strings[names[i]] = names_[i];
  }
  Handle(TDocStd_Document) doc = new TDocStd_Document("XmlXCAF");
  // In millimetres, the document's own unit, not OCCT's process-wide one.
  XCAFDoc_DocumentTool::SetLengthUnit(doc, 1.0, UnitsMethods_LengthUnit_Millimeter);
  Handle(XCAFDoc_ShapeTool) tool = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
  if (parts == 1) {
    set_name(tool->AddShape(given[0], Standard_False, Standard_False), names[0]);
  } else {
    const std::string root_name = placeholder("ASSEMBLY", 0);
    strings[root_name] = std::string(assembly.data(), assembly.size());
    const TDF_Label root = tool->NewShape();
    set_name(root, root_name);
    for (size_t i = 0; i < parts; ++i) {
      const TDF_Label component = tool->AddComponent(root, given[i], Standard_False);
      TDF_Label referred;
      if (XCAFDoc_ShapeTool::GetReferredShape(component, referred)) {
        set_name(referred, names[i]);
      }
      set_name(component, names[i]);
    }
    tool->UpdateAssemblies();
  }
  const std::string file_name_token = placeholder("FILE", 0);
  strings[file_name_token] = std::string(file_name.data(), file_name.size());

  DESTEP_Parameters params;
  params.WriteSchema = DESTEP_Parameters::WriteMode_StepSchema_AP214IS;
  params.WriteUnit = UnitsMethods_LengthUnit_Millimeter;
  // Pcurves written (OCCT's default; FreeCAD's off since 70c18c8017, which gives no reason):
  // without them a reader rebuilds them, and two spheres fused read back 1.1e-6 off in
  // volume, against 4.5e-11 with them (docs/ports/step-export.md, *Round trips*).
  params.WriteSurfaceCurMode = true;
  params.WriteAssembly = DESTEP_Parameters::WriteMode_Assembly_Auto;
  params.WritePrecisionMode = DESTEP_Parameters::WriteMode_PrecisionMode_Average;
  params.WriteProductName = "";
  STEPCAFControl_Writer writer;
  if (!writer.Transfer(doc, params, STEPControl_AsIs)) {
    throw StdFail_NotDone("OCCT's STEP writer couldn't transfer the parts");
  }
  STEPControl_Writer &plain = writer.ChangeWriter();
  const Handle(StepData_StepModel) model = plain.Model();
  const Handle(Transfer_FinderProcess) &process = plain.WS()->TransferWriter()->FinderProcess();

  // Each part's product's id, the feature's ID (OCCT writes the name twice); the assembly's
  // none.
  std::vector<Handle(StepBasic_Product)> products(parts);
  for (size_t i = 0; i < parts; ++i) {
    Handle(StepShape_ShapeDefinitionRepresentation) definition;
    const Handle(TransferBRep_ShapeMapper) mapper = TransferBRep::ShapeMapper(process, given[i]);
    if (!process->FindTypedTransient(mapper, STANDARD_TYPE(StepShape_ShapeDefinitionRepresentation),
                                     definition) ||
        definition.IsNull()) {
      throw StdFail_NotDone("OCCT's STEP writer wrote no product for a part");
    }
    const Handle(StepRepr_PropertyDefinition) property = definition->Definition().PropertyDefinition();
    if (property.IsNull()) {
      throw StdFail_NotDone("OCCT's STEP writer wrote a part's shape for no product");
    }
    const Handle(StepBasic_ProductDefinition) of = property->Definition().ProductDefinition();
    if (of.IsNull() || of->Formation().IsNull() || of->Formation()->OfProduct().IsNull()) {
      throw StdFail_NotDone("OCCT's STEP writer wrote a part's product incompletely");
    }
    products[i] = of->Formation()->OfProduct();
    products[i]->SetId(new TCollection_HAsciiString(ids_[i].c_str()));
  }
  for (int e = 1; e <= model->NbEntities(); ++e) {
    const Handle(StepBasic_Product) product = Handle(StepBasic_Product)::DownCast(model->Value(e));
    if (product.IsNull() ||
        std::find(products.begin(), products.end(), product) != products.end()) {
      continue;
    }
    product->SetId(new TCollection_HAsciiString(""));
  }
  // Each assembly link's id, its place among them, from 1: OCCT numbers them by a counter
  // that runs for the life of the process (STEPConstruct_Assembly::MakeRelationship).
  int links = 0;
  for (int e = 1; e <= model->NbEntities(); ++e) {
    const Handle(StepRepr_NextAssemblyUsageOccurrence) link =
        Handle(StepRepr_NextAssemblyUsageOccurrence)::DownCast(model->Value(e));
    if (!link.IsNull()) {
      link->SetId(new TCollection_HAsciiString(std::to_string(++links).c_str()));
    }
  }
  write_header(model, file_name_token);

  // The writer's check list: what it failed to transfer, but a face left out, and, as
  // diagnostics, what it warned of (it warns of what it corrects as it writes: "Direction of
  // Face of revolution corrected" on every cylinder).
  const Interface_CheckIterator checks = process->CheckList(Standard_False);
  for (checks.Start(); checks.More(); checks.Next()) {
    const Handle(Interface_Check) &check = checks.Value();
    if (check.IsNull()) {
      continue;
    }
    for (int n = 1; n <= check->NbWarnings(); ++n) {
      out->warnings_.push_back(check->CWarning(n));
    }
    if (!check->HasFailed()) {
      continue;
    }
    TopoDS_Shape shape;
    const Handle(TransferBRep_ShapeMapper) mapper =
        Handle(TransferBRep_ShapeMapper)::DownCast(check->Entity());
    if (!mapper.IsNull()) {
      shape = mapper->Value();
    }
    uint32_t part = UINT32_MAX;
    StepKind kind = StepKind::Other;
    uint32_t index = 0;
    for (size_t i = 0; i < parts && !shape.IsNull(); ++i) {
      int at = 0;
      if ((at = faces[i].FindIndex(shape)) > 0) {
        kind = StepKind::Face;
      } else if ((at = edges[i].FindIndex(shape)) > 0) {
        kind = StepKind::Edge;
      } else if ((at = vertices[i].FindIndex(shape)) > 0) {
        kind = StepKind::Vertex;
      } else if (!shapes_[i].IsSame(shape)) {
        continue;
      }
      part = static_cast<uint32_t>(i);
      index = static_cast<uint32_t>(at);
      break;
    }
    if (kind == StepKind::Face) {
      const TopAbs_Orientation o = faces[part](static_cast<int>(index)).Orientation();
      if (o == TopAbs_INTERNAL || o == TopAbs_EXTERNAL) {
        continue;
      }
    }
    std::string messages;
    for (int n = 1; n <= check->NbFails(); ++n) {
      messages += (messages.empty() ? "" : "\n");
      messages += check->CFail(n);
    }
    out->unwritten_parts_.push_back(part);
    out->unwritten_kinds_.push_back(kind);
    out->unwritten_indices_.push_back(index);
    out->unwritten_messages_.push_back(messages);
  }

  // The faces that bound material, and the face surfaces the file holds (an advanced face is
  // one; an oriented face refers to one), for the caller to compare: the writer drops a face
  // it can't transfer with only a warning (" No Wires of this Face were mapped").
  // And their bounds: the writer drops a wire it can't map from its face (" a Wire not
  // mapped"), a hole filled, with only a warning too.
  for (size_t i = 0; i < parts; ++i) {
    for (int f = 1; f <= faces[i].Extent(); ++f) {
      const TopAbs_Orientation o = faces[i](f).Orientation();
      if (o == TopAbs_INTERNAL || o == TopAbs_EXTERNAL) {
        continue;
      }
      ++out->faces_expected_;
      for (TopExp_Explorer wire(faces[i](f), TopAbs_WIRE); wire.More(); wire.Next()) {
        ++out->bounds_expected_;
      }
    }
  }
  for (int e = 1; e <= model->NbEntities(); ++e) {
    const Handle(Standard_Transient) &entity = model->Value(e);
    if (entity->IsKind(STANDARD_TYPE(StepShape_FaceSurface))) {
      ++out->faces_written_;
    } else if (entity->IsKind(STANDARD_TYPE(StepShape_FaceBound))) {
      ++out->bounds_written_;
    }
  }

  std::ostringstream stream;
  if (writer.WriteStream(stream) != IFSelect_RetDone) {
    throw StdFail_NotDone("OCCT's STEP writer couldn't write the file");
  }
  const std::string text = spliced(stream.str(), strings);
  out->bytes_.assign(text.begin(), text.end());
  return out;
}

} // namespace silico
