//! nyx-engine: the only crate that talks to libtorrent. Exposes a safe Rust API
//! over the C++ shim in `cpp/`; everything above this crate stays libtorrent-free.

mod ffi;

/// Returns the linked libtorrent-rasterbar version, e.g. "2.0.11.0".
/// Used by the toolchain spike and for the About screen.
pub fn libtorrent_version() -> String {
    ffi::lt_version()
}

/// Starts a real libtorrent session (loopback only, no DHT/LSD/UPnP/NAT-PMP)
/// and shuts it down. Proves the full engine links and runs, not just headers.
pub fn session_smoke() -> bool {
    ffi::lt_session_smoke()
}
