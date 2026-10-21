//! Toolchain spike test: proves libtorrent-rasterbar is statically linked into
//! a Rust binary through the cxx shim and that the expected 2.0.x line is used.

/// Calls into libtorrent via the shim; a link or version mismatch fails here.
#[test]
fn libtorrent_version_is_2_0() {
    let v = nyx_engine::libtorrent_version();
    assert!(v.starts_with("2.0"), "unexpected libtorrent version: {v}");
}

/// Builds and tears down a real lt::session, which pulls most of libtorrent,
/// boost.asio and openssl into the link, so the dependency check is meaningful.
#[test]
fn session_starts_and_stops() {
    assert!(nyx_engine::session_smoke());
}
