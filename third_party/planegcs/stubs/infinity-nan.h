// Force-included (-include) before every PlaneGCS source. In C++20, clang reports
// __has_feature(modules), and then the macOS SDK's math.h leaves INFINITY and NAN to
// <float.h>'s __need_infinity_nan, which the pinned libc++'s float.h doesn't pass on: so
// libc++'s own <complex>, which Eigen includes, doesn't compile. These are the SDK's own
// definitions, used only when math.h didn't make them. (Linux's math.h defines both.)
#ifndef SILICO_PLANEGCS_INFINITY_NAN_H
#define SILICO_PLANEGCS_INFINITY_NAN_H
#include <math.h>
#ifndef INFINITY
#define INFINITY __builtin_inff()
#endif
#ifndef NAN
#define NAN __builtin_nanf("0x7fc00000")
#endif
#endif
