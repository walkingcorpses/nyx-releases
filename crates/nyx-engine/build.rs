//! Build script for nyx-engine: locates the static libtorrent (and its boost and
//! openssl deps) installed by vcpkg, then compiles the C++ shim and cxx bridge
//! against it as C++17.

/// Finds libtorrent via vcpkg (emits link lines for it and its deps) and builds the shim.
fn main() {
    // With +crt-static the vcpkg crate selects the x64-windows-static triplet.
    let lt = vcpkg::Config::new()
        .find_package("libtorrent")
        .expect("libtorrent not found in vcpkg; run scripts/setup-toolchain.ps1");

    let mut build = cxx_build::bridge("src/ffi.rs");
    build.file("cpp/shim.cpp").std("c++17").includes(&lt.include_paths);
    // Must match libtorrent's INTERFACE_COMPILE_DEFINITIONS (LibtorrentRasterbarTargets.cmake)
    // or shim code sees different struct layouts than the prebuilt library.
    for def in [
        "BOOST_ASIO_ENABLE_CANCELIO", "BOOST_ASIO_NO_DEPRECATED", "WIN32_LEAN_AND_MEAN",
        "BOOST_ALL_NO_LIB", "_SCL_SECURE_NO_DEPRECATE", "_CRT_SECURE_NO_DEPRECATE",
        "TORRENT_NO_DEPRECATE", "TORRENT_USE_OPENSSL", "TORRENT_USE_LIBCRYPTO",
        "TORRENT_SSL_PEERS", "OPENSSL_NO_SSL2",
    ] {
        build.define(def, None);
    }
    build.flag("/EHsc").flag("/bigobj");
    build.compile("nyx-shim");

    // Windows system libraries libtorrent/boost.asio/openssl need for a static link;
    // the vcpkg crate only emits the vcpkg-built libs.
    for lib in ["ws2_32", "mswsock", "iphlpapi", "crypt32", "bcrypt", "advapi32", "user32", "dbghelp"] {
        println!("cargo:rustc-link-lib={lib}");
    }

    println!("cargo:rerun-if-changed=cpp/shim.cpp");
    println!("cargo:rerun-if-changed=cpp/shim.hpp");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
}
