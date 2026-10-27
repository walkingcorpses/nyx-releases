//! Build script for nyx-engine: locates the static libtorrent (and its boost and
//! openssl deps) installed by vcpkg, then compiles the C++ shim and cxx bridge
//! against it as C++17. Builds on Windows (MSVC, the app) and Linux (gcc, the
//! nyx-daemon Docker image in lab/images/nyx-daemon.Dockerfile).

/// Finds libtorrent via vcpkg (emits link lines for it and its deps) and builds the shim.
fn main() {
    // Target OS, not host OS: build scripts run on the host but configure the target.
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");

    // With +crt-static the vcpkg crate selects the x64-windows-static triplet;
    // on Linux it selects x64-linux (static libs by default).
    let lt = vcpkg::Config::new()
        .find_package("libtorrent")
        .expect("libtorrent not found in vcpkg; run scripts/setup-toolchain.ps1");

    let mut build = cxx_build::bridge("src/ffi.rs");
    build.file("cpp/shim.cpp").std("c++17").includes(&lt.include_paths);
    // Must match libtorrent's INTERFACE_COMPILE_DEFINITIONS (LibtorrentRasterbarTargets.cmake)
    // or shim code sees different struct layouts than the prebuilt library.
    // The x64-linux build lists the same set minus the four Windows-only defines below.
    const WINDOWS_ONLY: [&str; 4] =
        ["WIN32_LEAN_AND_MEAN", "BOOST_ALL_NO_LIB", "_SCL_SECURE_NO_DEPRECATE", "_CRT_SECURE_NO_DEPRECATE"];
    for def in [
        "BOOST_ASIO_ENABLE_CANCELIO", "BOOST_ASIO_NO_DEPRECATED", "WIN32_LEAN_AND_MEAN",
        "BOOST_ALL_NO_LIB", "_SCL_SECURE_NO_DEPRECATE", "_CRT_SECURE_NO_DEPRECATE",
        "TORRENT_NO_DEPRECATE", "TORRENT_USE_OPENSSL", "TORRENT_USE_LIBCRYPTO",
        "TORRENT_SSL_PEERS", "OPENSSL_NO_SSL2",
    ] {
        if windows || !WINDOWS_ONLY.contains(&def) {
            build.define(def, None);
        }
    }
    if windows {
        build.flag("/EHsc").flag("/bigobj");
    }
    build.compile("nyx-shim");

    // System libraries libtorrent/boost.asio/openssl need for a static link;
    // the vcpkg crate only emits the vcpkg-built libs.
    let system_libs: &[&str] = if windows {
        &["ws2_32", "mswsock", "iphlpapi", "crypt32", "bcrypt", "advapi32", "user32", "dbghelp"]
    } else {
        // Threads::Threads and openssl's -ldl (from libtorrent-rasterbar.pc).
        &["pthread", "dl"]
    };
    for lib in system_libs {
        println!("cargo:rustc-link-lib={lib}");
    }

    println!("cargo:rerun-if-changed=cpp/shim.cpp");
    println!("cargo:rerun-if-changed=cpp/shim.hpp");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-changed=../../vcpkg.json");
}
