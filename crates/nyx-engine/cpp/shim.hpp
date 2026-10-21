// shim.hpp - C++ side of the cxx bridge between Rust and libtorrent-rasterbar.
// Keeps libtorrent types out of the Rust API: only cxx-friendly types cross here.
#pragma once

#include "rust/cxx.h"

namespace nyx {

// Returns the linked libtorrent version string (lt::version()), e.g. "2.0.10.0".
rust::String lt_version();

} // namespace nyx
