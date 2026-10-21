// shim.hpp - C++ side of the cxx bridge between Rust and libtorrent-rasterbar.
// Keeps libtorrent types out of the Rust API: only cxx-friendly types cross here.
#pragma once

#include "rust/cxx.h"

namespace nyx {

// Returns the linked libtorrent version string (lt::version()), e.g. "2.0.10.0".
rust::String lt_version();

// Starts an lt::session on loopback with DHT/LSD/UPnP/NAT-PMP off, reads its
// settings back, then destroys it. True when the round trip works.
bool lt_session_smoke();

} // namespace nyx
