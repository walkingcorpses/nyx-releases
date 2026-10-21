# Nyx: Design Spec

Date: 2026-10-21
Status: Draft, awaiting owner review

## 1. Purpose

Nyx is a Windows desktop torrent client for the owner's personal use. It ships as a single portable `nyx.exe` with no installer. It adds torrents from `.torrent` files or magnet links, enforces encrypted peer traffic, and has a polished dark UI.

### Success criteria

- One `nyx.exe` runs on Windows 11 with nothing else installed (WebView2 ships with Windows 11).
- A magnet link or `.torrent` file added through any entry point downloads to completion.
- With forced encryption on, the client never completes a plaintext peer connection.
- With a SOCKS5 proxy configured, no peer, tracker or DHT traffic leaves outside the proxy.
- The UI is dark-only, custom-styled, and responsive with hundreds of torrents.

### Out of scope for v1

VPN kill-switch, password-locked app and encrypted local data, RSS auto-download, built-in torrent search, macOS and Linux builds.

## 2. Architecture

| Layer | Choice | Responsibility |
|---|---|---|
| Shell | Tauri 2 | Window, tray, single-instance, file and `magnet:` association, notifications, embeds the UI into the exe |
| Backend | Rust (`src-tauri`) | Tauri commands and events, settings, scheduler, queue logic |
| Engine | libtorrent-rasterbar, statically linked, wrapped in crate `nyx-engine` | All torrent work. Exposes a small Rust API; nothing else touches libtorrent |
| UI | Svelte + TypeScript, custom CSS | Rendering and user input only. No torrent logic |

### Engine API (`nyx-engine`)

`add_magnet(uri, opts)`, `add_torrent_file(bytes, opts)`, `pause(id)`, `resume(id)`, `remove(id, delete_files)`, `set_file_priorities(id, priorities)`, `set_sequential(id, bool)`, `set_limits(global_or_id, down, up)`, `set_encryption(mode)`, `set_proxy(cfg)`, `snapshot()` (all torrent states), `torrent_detail(id)` (files, peers, trackers).

The wrapper is the only unsafe-FFI boundary. The rest of the app is safe Rust.

### Data flow

The backend polls `snapshot()` about once per second and emits a state event to the UI. The UI sends user actions as Tauri commands. Events are diffed so only changed torrents are sent.

### Persistence

Everything lives in `%APPDATA%\Nyx`: `settings.json`, libtorrent session state and per-torrent resume data, and the category list. A portable build never writes next to the exe unless a `nyx.portable` marker file is present beside it.

## 3. Features (v1)

- **Add torrents:** file picker button, drag and drop onto the window, double-click or "open with" association for `.torrent`, `magnet:` URL handler, paste a magnet into an add dialog. A pre-download dialog shows the file tree with checkboxes, save folder, category and a sequential toggle.
- **Manage:** pause, resume, remove, remove and delete files, force recheck, open folder, copy magnet.
- **Queue:** configurable max active downloads and seeds. Excess torrents wait in a queue that can be reordered.
- **Categories:** named categories, each with a default save folder.
- **Limits:** global and per-torrent download and upload caps. A weekly scheduler of time blocks overrides the global caps.
- **System integration:** tray icon with minimize-to-tray, completion notifications, start minimized option, and single-instance behavior so a magnet click opens in the running app.
- **Detail drawer:** files with per-file progress, peers (address, client, speed, flags, encrypted yes/no), trackers, and a speed graph.

## 4. Security

### Peer encryption

Default is forced: libtorrent `pe_forced` with RC4 only, so plaintext peers are refused. A setting offers "prefer encrypted" for hard-to-seed torrents. The peers table shows whether each connection is encrypted.

### Proxy

SOCKS5 with optional username and password, applied to peers, trackers and DHT. "Proxy peer connections" and "proxy hostname lookups" are on, and local peer discovery is off while a proxy is active, to prevent leaks. If a proxy is configured but unreachable, torrents do not fall back to a direct connection.

### Application hardening

- Strict Content Security Policy, no remote content in the webview, no remote scripts.
- Minimal Tauri capability set. The UI can call only the named commands.
- `.torrent` and magnet parsing handle untrusted input, with size and depth limits, and fuzz tests on the parser entry points.
- Save paths are validated so a torrent cannot write outside its target folder.
- No telemetry, no network calls except torrent traffic.

### Stated limits

Message stream encryption (MSE) hides payload content and defeats simple traffic-pattern blocking. It is obfuscation, not authentication, and it does not hide the user's IP address from other peers in a swarm. Only a VPN or proxy does that. The UI settings page says so plainly.

## 5. Look and feel

Dark only. Near-black base, violet and cyan accents, glass-style panels, a smooth animated progress bar per torrent, a live speed graph, a compact table as the main view with a detail drawer, and subtle motion. Custom title bar matched to the theme.

## 6. Testing

- **Engine wrapper:** Rust tests against a local tracker and seeder on loopback.
- **Parsing:** unit tests for magnet and `.torrent` handling, plus fuzz targets.
- **Encryption:** a test that a plaintext-only peer is refused in forced mode and accepted in "prefer" mode.
- **Proxy:** a test with a local SOCKS5 server confirming all traffic goes through it, and that nothing connects directly when the proxy is down.
- **Scheduler and queue:** pure-logic unit tests.
- **UI:** Playwright smoke tests against the built exe, run off-screen so they do not disturb the owner.

## 7. Build prerequisites

Rust via rustup, Visual Studio MSVC C++ build tools, CMake, and vcpkg for libtorrent-rasterbar, OpenSSL and Boost. Statically link everything so the output is one exe. The first build is long. Exe size target is under 30 MB.

## 8. Risks

- **libtorrent static build on Windows:** the largest unknown. The first plan task is a build spike that produces a hello-world exe linking libtorrent, before any feature work.
- **Rust to C++ bridge:** use the `cxx` crate with a thin C++ shim over libtorrent, so the unsafe surface stays small.
- **WebView2 absent:** Windows 11 includes it. The exe should show a clear message if it is missing.

## 9. Code conventions

Every file and function carries comments: a header per file, and the what and why on each function. The owner edits by hand.
