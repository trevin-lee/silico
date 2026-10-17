// Silico's stand-in for FreeCAD's FCGlobal.h, which SketcherGlobal.h includes for the
// export macros PlaneGCS's classes are declared with. PlaneGCS is linked statically into
// the sketch worker, so they're empty. (FreeCAD's own pulls in Qt.)
#ifndef FC_GLOBAL_H
#define FC_GLOBAL_H
#define FREECAD_DECL_EXPORT
#define FREECAD_DECL_IMPORT
#endif
