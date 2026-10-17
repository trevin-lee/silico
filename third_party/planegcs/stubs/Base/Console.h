// Silico's stand-in for FreeCAD's Base/Console.h, the one FreeCAD header PlaneGCS's
// GCS.cpp uses: its log and warning calls. They're debugging output (even at the default
// debug level, each redundancy solve logs two lines), so they're dropped: a worker's
// stderr carries only its JSON log lines.
#ifndef SILICO_PLANEGCS_BASE_CONSOLE_H
#define SILICO_PLANEGCS_BASE_CONSOLE_H

namespace Base {

class ConsoleSingleton {
public:
    template <typename... Args> void log(const Args&...) {}
    template <typename... Args> void warning(const Args&...) {}
};

inline ConsoleSingleton& Console() {
    static ConsoleSingleton console;
    return console;
}

}  // namespace Base

#endif
