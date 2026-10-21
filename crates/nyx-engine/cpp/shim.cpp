// shim.cpp - implementation of the cxx bridge functions declared in shim.hpp.
// Every libtorrent call Nyx makes goes through this file.
#include "nyx-engine/cpp/shim.hpp"

#include <libtorrent/version.hpp>

namespace nyx {

// Calls lt::version() (a compiled function, not a header macro) so the
// spike proves the static library actually links, not just that headers exist.
rust::String lt_version() {
    return rust::String(lt::version());
}

} // namespace nyx
