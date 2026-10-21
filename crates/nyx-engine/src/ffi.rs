//! cxx bridge declarations: the raw C++ functions exported by `cpp/shim.cpp`.
//! Kept private to the crate; `lib.rs` wraps each one in a safe public API.

#[cxx::bridge(namespace = "nyx")]
mod bridge {
    unsafe extern "C++" {
        include!("nyx-engine/cpp/shim.hpp");

        /// libtorrent version string from `lt::version()`.
        fn lt_version() -> String;

        /// Starts and stops a loopback-only `lt::session`; true on success.
        fn lt_session_smoke() -> bool;
    }
}

pub use bridge::*;
