<h1 align="center">Nyx</h1>

<p align="center">
  A fast, private torrent client for Windows.<br>
  One portable <code>.exe</code>. Forced peer encryption, SOCKS5 proxy with a kill switch, dark UI. No installer, no telemetry.
</p>

<p align="center">
  <b>Nyx is open source (MIT).</b> The full source code is in this repository, and every release is tagged so you can read exactly what was built.<br>
  <a href="#build-from-source">Build it yourself</a> · <a href="https://github.com/walkingcorpses/nyx-releases/issues">Report a bug or request a feature</a>
</p>

<p align="center">
  <a href="https://github.com/walkingcorpses/nyx-releases/releases/latest"><b>Download the latest version</b></a>
</p>

<p align="center">
  <img src="images/main.png" alt="Nyx main window: torrent table with queue positions, progress bars and a live speed graph" width="900">
</p>

---

## Download

1. Open the **[latest release](https://github.com/walkingcorpses/nyx-releases/releases/latest)** and download **`Nyx-<version>-windows-x64.exe`**.
2. Put it anywhere you like and double-click it. There is nothing to install.

**Needs:** Windows 11 (Windows 10 works too if the Microsoft Edge *WebView2 Runtime* is installed; Windows 11 has it built in). 64-bit only.

### Check the download (optional but easy)

Every release lists a SHA-256 next to the file, and ships a `.sha256` file. In PowerShell:

```powershell
Get-FileHash .\Nyx-1.0.0-windows-x64.exe -Algorithm SHA256
```

The hash must match the one on the release page.

### "Windows protected your PC"

Nyx is not code-signed (a certificate costs money and this is a hobby project), so Windows SmartScreen may warn the first time. Click **More info**, then **Run anyway**. The checksum above is how you know you got the file that was published.

## What you get

| | |
|---|---|
| **Adding** | `.torrent` files (button, drag and drop, "open with") and magnet links (paste anywhere, click a magnet link). A file tree lets you pick what to download before it starts, plus save folder, category and "download in order". |
| **Managing** | Pause, resume, remove (optionally with files), recheck, open folder, copy magnet. Drag the grip in the **#** column to reorder the queue. |
| **Queue & limits** | Max active downloads and seeds, global and per-torrent speed limits, and a weekly schedule that swaps in different limits by day and time. |
| **Categories** | Each category has its own folder; the add dialog switches to it automatically. |
| **Everywhere** | Tray icon, notifications when a download finishes, single instance (clicking a magnet while Nyx is hidden in the tray opens it in the running app). |

<p align="center">
  <img src="images/add.png" alt="Add dialog with file tree" width="720">
</p>

## Privacy and security

Nyx is built so that "private" is the default, and where it can only go so far, it says so.

- **Peer encryption is forced by default.** Peers that do not encrypt are refused. A setting lets you accept plaintext peers for hard-to-seed torrents.
- **SOCKS5 proxy for everything.** Peers, trackers and hostname lookups go through your proxy. While a proxy is on, Nyx also **turns off DHT, local peer discovery, UPnP and NAT-PMP, and refuses direct inbound connections**, so nothing can go around it. If the proxy is down, torrents do **not** fall back to a direct connection.
- **Kill switch.** With a proxy set, Nyx checks it every few seconds. If it cannot be reached, every torrent is paused and a banner tells you; they resume by themselves when it is back.
- **Bind to one network adapter** (for example your VPN). Nyx then listens and connects only through that adapter, so if the VPN drops, nothing connects.
- **IP blocklist.** Load a PeerGuardian, plain-range or CIDR list and those addresses are refused in both directions.
- **No telemetry.** The only network traffic is torrent traffic. The optional extras below (update check, RSS, search) are off or empty until you set them, and they also go through your proxy.
- **Hardened app shell.** No remote content in the window, a strict content security policy, and the interface can only call the handful of commands it needs. Torrent files and magnet links are treated as untrusted input; a torrent cannot write outside its save folder.

<p align="center">
  <img src="images/settings-connection.png" alt="Settings: peer encryption, proxy, kill switch, adapter, blocklist" width="720">
</p>

**What encryption does and does not do.** Message stream encryption hides *what* you transfer and defeats simple traffic-pattern blocking. It is obfuscation, not authentication, and it **does not hide your IP address from the other peers in a swarm**. Only a VPN or proxy does that, which is why Nyx makes them easy to use properly.

**What was tested.** Nyx is tested against the real protocol, not just mocked: sessions seeding to each other on loopback, a plaintext-only peer that forced mode must refuse, a SOCKS5 test server that proves peer connections, UDP tracker announces and inbound connections behave as promised (and that nothing leaks when the proxy is down), plus a real download through the proxy from a public swarm. One thing is *not* covered by an automated test: the adapter binding was only tested against an adapter that does not exist, not a live VPN.

## Built-in VPN (WireGuard)

Settings > Network lets you use a VPN without installing anything else and without admin rights.

- **Import a WireGuard config** (the `.conf` file your provider gives you). Nyx runs the tunnel inside the app, so **only Nyx's traffic** goes through it. Your private key is stored encrypted for your Windows account and is never shown again.
- **Network lock.** If the VPN is not really connected, every torrent is paused and web requests (RSS, search, update check) are blocked. Nyx judges "connected" by a handshake that *it* started and the server completed, not by packets merely arriving, so a half-working connection does not count. On start the lock holds until the first handshake.
- **Rotation (off by default).** Pick two or more profiles and Nyx switches between them every 5 to 10 minutes (you choose the range). The new tunnel is brought up first, and only then does Nyx move over. If it cannot connect, the lock holds; Nyx never falls back to the old route or to a direct connection. Every change of route (a rotation, or picking another profile or "No VPN" in Settings) also gives the torrent engine a new peer ID and tracker key, so trackers cannot link sessions across servers. If renewing the identity fails, Nyx shows a warning and carries on with the old one. Switching resets your peer connections, and some private trackers ban for changing IP, so leave rotation off for those.
- **What this does not do.** Other peers in a swarm still see your VPN exit address, and your VPN provider can still see your traffic. Tor is not included yet. The tunnel carries no inbound connections, so you only connect out.

**Proof, not promises.** The repo has a leak lab (`lab/`): Docker containers with Nyx clients, VPN exits, a tracker, seeders and a packet capture on each client. It checks that the only thing leaving the client is WireGuard to the active server, that no server ever sees the real address, that the lock holds when the tunnel is cut (one way, both ways, for minutes), during rotation and across crashes, and that sessions cannot be linked across rotations (new peer ID and tracker key each time, and the torrent engine sends no client name or version; Nyx's own web requests for feeds, search and the update check still say "Nyx"). Traits shared by every libtorrent client, and timing correlation between the old and new identity, can still single out a libtorrent client. A deliberate-leak control proves the lab can see leaks. Run it with `labun.ps1` (about 15 minutes, needs Docker). It does not cover your VPN provider's logging, timing correlation by peers, or Windows-only behaviour such as adapter binding.

## Automation (all optional)

<p align="center">
  <img src="images/settings-automation.png" alt="Settings: RSS feeds and search provider" width="720">
</p>

- **RSS auto-download.** Add feeds with a title filter (a case-insensitive regular expression) and an optional category. The first time a feed is polled Nyx only *records* what is already there, so adding a feed never floods your downloads; after that, new matching items are added and started in the category's folder. Items are remembered across restarts.
- **Built-in search.** Nyx does not ship with a search provider or any feeds. You point it at an RSS/Torznab address that contains `{query}` (for example your own Jackett or Prowlarr), and the **Search…** button searches it. Pick a result and it opens in the normal add dialog.
- **Updates.** Off by default. Switch on *Check for a newer Nyx when it starts* (or press *Check now*) and Nyx asks this repository for the latest release. An update is only installed after its **SHA-256 matches the one GitHub reports**, from this repository only; the old exe is kept as `nyx.exe.old` until the next start. Nothing is installed without you pressing *Update now*.

<p align="center">
  <img src="images/search.png" alt="Search dialog with results" width="720">
</p>

## Where is my data?

In `%APPDATA%\Nyx` (settings, session state, resume data, RSS history). Want it portable? Create an empty file named **`nyx.portable`** next to `Nyx-….exe` and Nyx keeps everything in that folder instead. Delete the folder to remove every trace.

To make Nyx open `.torrent` files and magnet links: **Settings → System → Make Nyx the default** (your account only, no admin rights needed).

## FAQ

**Why does the DHT stop when I use a proxy?** DHT is UDP traffic to arbitrary nodes; letting it run would be a way around the proxy. With a proxy set, Nyx finds peers through trackers (which do go through the proxy).

**Does the kill switch protect me if I use a VPN instead of a proxy?** The kill switch watches the SOCKS5 proxy. For a VPN, use *Network adapter* binding: Nyx connects only through that adapter.

**Is it safe to automate downloads?** Nyx never runs anything it downloads and only ever adds torrents. You are responsible for what your feeds and search provider point at.

**Where is the source code?** Right here: this repository holds the source and every release is tagged. See [Build from source](#build-from-source). MIT licensed.

## Build from source

The source is in this repository (MIT). Design notes: `docs/superpowers/specs/2026-10-21-nyx-design.md`; plan and deviations: `docs/superpowers/plans/2026-10-21-nyx.md`. Each release is tagged `vX.Y.Z`, so the **Source code** archive on a release page is exactly what was built.

### Layout

```
crates/nyx-core     pure logic: input validation, queue, categories, schedule, settings (no libtorrent)
crates/nyx-engine   safe Rust API over libtorrent (cpp/shim.cpp is the only C++); testkit behind `test-support`
src-tauri           the app: commands, 1 Hz poller, tray, single instance, file associations
ui                  Svelte + TypeScript UI (no torrent logic)
e2e                 Playwright smoke test that drives the real exe
scripts             setup-toolchain.ps1, build-release.ps1
```

### Build prerequisites

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

### Environment

- `VCPKG_ROOT`: vcpkg location. The script sets it as a user env var. `.cargo/config.toml` falls back to
  `D:\Projects\tools\vcpkg` when it is unset, so a plain shell builds without extra setup.
- `.cargo/config.toml` also sets `+crt-static`, which statically links the C runtime and makes the build use the
  `x64-windows-static` vcpkg triplet.
- The vcpkg Rust crate only reads `%VCPKG_ROOT%\installed`, so the script installs the manifest there.
  Manifest mode removes packages from that tree that `vcpkg.json` does not list.

### Build and run

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1   # builds the UI + dist\nyx.exe, checks its DLLs
dist\nyx.exe
```

Data lives in `%APPDATA%\Nyx`. If a file named `nyx.portable` sits next to `nyx.exe`, data lives next to the exe instead.
Settings has a "Make Nyx the default" button for `.torrent` files and magnet links (current user only, no admin).

### Test

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

### Publishing a release

Bump `version` in `Cargo.toml` and `src-tauri/tauri.conf.json`, commit and push, run `scripts\build-release.ps1`, then `scripts\publish-release.ps1 -NotesFile notes.md`. It tags the commit and uploads `Nyx-X.Y.Z-windows-x64.exe` plus its `.sha256`; the in-app updater depends on those names.

## Please use it lawfully

Nyx is a neutral BitTorrent client, like any other. Only download and share content you have the right to. It ships with no search provider, no feeds and no trackers of its own.

## What's new

### 1.0.0

First public release. The source is open and lives in this repository. Everything above: forced encryption, SOCKS5 proxy with leak protection and kill switch, adapter binding, IP blocklist, queue with drag-to-reorder, categories, schedule, RSS auto-download, search, opt-in checksummed self-update, tray, single instance and file associations.

## Credits

Built with [libtorrent-rasterbar](https://libtorrent.org/), [Tauri](https://tauri.app/), [Rust](https://www.rust-lang.org/) and [Svelte](https://svelte.dev/).

MIT licensed.
