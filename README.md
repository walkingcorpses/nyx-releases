<!-- README.md - Nyx: Windows torrent client (Tauri + Rust + libtorrent). Build prerequisites and setup. -->
# Nyx

Windows torrent client: Tauri 2 + Rust, with libtorrent-rasterbar 2.0.x statically linked through a `cxx` shim.
The output is one portable, statically linked exe that depends only on system DLLs.

## Build prerequisites

Run once from PowerShell (the Build Tools step may ask for UAC):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-toolchain.ps1
```

It installs, skipping anything already present:

- Rust via rustup, toolchain `stable-x86_64-pc-windows-msvc`
- Visual Studio 2022 Build Tools, "Desktop development with C++" workload
- CMake
- vcpkg, cloned to `D:\Projects\tools\vcpkg`
- libtorrent 2.0.11 (pinned in `vcpkg.json`) plus boost and openssl, triplet `x64-windows-static`

## Environment

- `VCPKG_ROOT`: vcpkg location. The script sets it as a user env var. `.cargo/config.toml` falls back to
  `D:\Projects\tools\vcpkg` when it is unset, so a plain shell builds without extra setup.
- `.cargo/config.toml` also sets `+crt-static`, which statically links the C runtime and makes the build use the
  `x64-windows-static` vcpkg triplet.
- The vcpkg Rust crate only reads `%VCPKG_ROOT%\installed`, so the script installs the manifest there.
  Manifest mode removes packages from that tree that `vcpkg.json` does not list.

## Build and test

```powershell
cargo test -p nyx-engine
cargo build --release
```
