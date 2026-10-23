<!-- README.md - Nyx: Windows torrent client (Tauri + Rust + libtorrent). What it is, how to build, test and run it. -->
# Nyx

Windows torrent client: Tauri 2 + Rust, with libtorrent-rasterbar 2.0.x statically linked through a `cxx` shim and a
Svelte UI. The output is one portable `nyx.exe` (about 13 MB) that depends only on Windows system DLLs. It needs the
WebView2 runtime, which ships with Windows 11.

- Add torrents from `.torrent` files (picker, drag and drop, "open with") or magnet links (paste, magnet click),
  with a pre-download file tree, save folder, category and sequential option.
- Peer encryption is forced by default (refuses plaintext peers). Optional SOCKS5 proxy for peers, trackers and
  hostnames; no fallback to a direct connection, DHT/LSD/UPnP/NAT-PMP off and no direct inbound connections while
  it is on. Kill switch (pauses everything when the proxy is unreachable), adapter binding for VPNs, IP blocklist.
- Queue with drag-to-reorder, categories, global and per-torrent limits, a weekly speed schedule, tray icon,
  notifications, single instance.
- RSS auto-download (new feeds are baselined, never flooded), search through a provider you configure, opt-in
  self-update that installs only a download whose SHA-256 matches the release.
- Dark-only UI with a live speed graph; the table stays smooth with a thousand torrents.
- No telemetry. The only network traffic is torrent traffic plus the optional update check, RSS and search, which
  all follow the proxy setting.

Nyx is open source (MIT). Downloads and the user-facing README live in `walkingcorpses/nyx-releases`; each release there is tagged `vX.Y.Z` in this repo and carries a matching source zip.
Publish a release with `scripts\publish-release.ps1` (tag `vX.Y.Z`, asset `Nyx-X.Y.Z-windows-x64.exe`: the in-app
updater depends on those names).

Design: `docs/superpowers/specs/2026-10-21-nyx-design.md`. Plan and deviations: `docs/superpowers/plans/2026-10-21-nyx.md`.

## Layout

```
crates/nyx-core     pure logic: input validation, queue, categories, schedule, settings (no libtorrent)
crates/nyx-engine   safe Rust API over libtorrent (cpp/shim.cpp is the only C++); testkit behind `test-support`
src-tauri           the app: commands, 1 Hz poller, tray, single instance, file associations
ui                  Svelte + TypeScript UI (no torrent logic)
e2e                 Playwright smoke test that drives the real exe
scripts             setup-toolchain.ps1, build-release.ps1
```

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

Node.js (any current LTS) is needed for the UI.

## Environment

- `VCPKG_ROOT`: vcpkg location. The script sets it as a user env var. `.cargo/config.toml` falls back to
  `D:\Projects\tools\vcpkg` when it is unset, so a plain shell builds without extra setup.
- `.cargo/config.toml` also sets `+crt-static`, which statically links the C runtime and makes the build use the
  `x64-windows-static` vcpkg triplet.
- The vcpkg Rust crate only reads `%VCPKG_ROOT%\installed`, so the script installs the manifest there.
  Manifest mode removes packages from that tree that `vcpkg.json` does not list.

## Build and run

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1   # builds the UI + dist\nyx.exe, checks its DLLs
dist\nyx.exe
```

Data lives in `%APPDATA%\Nyx`. If a file named `nyx.portable` sits next to `nyx.exe`, data lives next to the exe instead.
Settings has a "Make Nyx the default" button for `.torrent` files and magnet links (current user only, no admin).

## Test

```powershell
cargo test --workspace            # core, engine (real libtorrent sessions on loopback), app, instance tests
cargo test -p nyx-engine --test swarm -- --ignored --nocapture   # real public swarm, direct and via proxy (needs internet)
npm --prefix ui test              # UI unit and component tests (Vitest)
npm --prefix ui run check         # svelte-check
cd e2e; npx playwright test       # smoke test of the built exe (run build-release.ps1 first)
```

`src-tauri/tests/instance.rs` and the e2e test start the real exe. They run off-screen and muted, with their own data
folder and WebView2 profile, and only ever kill the process they started. They use these environment variables, which
the app reads in every run but which only matter for tests: `NYX_DATA_DIR`, `NYX_LISTEN`, `NYX_NO_DISCOVERY`,
`NYX_TEST_OFFSCREEN`, `NYX_TEST_LOG` (also enables a file-triggered tray action for the e2e test), and `NYX_EXE`
(path of the exe under test).

UI development without the backend: `npm --prefix ui run dev` serves the UI with an in-memory mock
(`?rows=1000` loads a thousand rows).
