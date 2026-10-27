# Network Privacy (VPN, Rotation, Leak Lab) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hook up WireGuard VPNs from the Nyx UI behind a network lock, rotate between them on a timer, and prove with a Dockerised leak lab that Nyx leaks neither the real IP nor a linkable identity.

**Architecture:** A new `nyx-tunnel` crate runs userspace WireGuard (`boringtun` + `smoltcp`) and exposes a local password-protected SOCKS5 port; libtorrent and the HTTP agent use only that port, with `outgoing_interfaces` bound to `127.0.0.1`. The app logic (`state.rs` and friends) moves out of `src-tauri` into `nyx-app`, which owns the network lock and rotation and also powers a headless `nyx-daemon` that the lab runs in Docker.

**Tech Stack:** Rust (existing workspace), `boringtun`, `smoltcp`, `base64`, `rand`, Windows DPAPI via `windows-sys`, libtorrent 2.0.11 (existing), Svelte (existing UI), Docker Compose + Python 3 (`dpkt`) for the lab.

**Spec:** `docs/superpowers/specs/2026-10-27-network-privacy-design.md`

**Scope note:** This plan covers spec build-order steps 1 to 5. Steps 6 and 7 (Tor via `arti`, `pktmon` smoke test on `nyx.exe`) get their own plan after this one ships, because the `arti` size check and the lab results should inform them.

## Global Constraints

- Work on branch `feat/network-privacy`. Do not push. Every commit message ends with the line `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Every new file starts with a header comment saying what it is for, and every new function has a comment saying what and why (owner edits by hand).
- No admin rights and no kernel driver. Only Nyx traffic goes through the tunnel.
- With a profile active: DHT, uTP, LSD, UPnP and NAT-PMP off, listener on loopback only, and `outgoing_interfaces` = `127.0.0.1` (all already true in proxy mode except the interface).
- Local SOCKS5 server: `127.0.0.1`, random port, per-session username and password.
- Tunnel is down when no WireGuard handshake has completed within 180 seconds. The lock is on by default and reuses the existing `kill_switch` setting.
- A profile is one peer with `AllowedIPs` covering `0.0.0.0/0` and `::/0`. Anything else is rejected at import with a specific message.
- Private keys are sealed with Windows DPAPI (stored as given on non-Windows) and never logged, never returned to the UI, never in lab captures.
- Rotation: off by default; random interval between a minimum and maximum, default 5 and 10 minutes; next profile is random and never the current one; make-before-break; a failed swap holds torrents and never falls back to the old route or to direct.
- Windows build and the existing test suites stay green after every task (`cargo test --workspace`, UI `npm test`). Tests never kill processes by name.

## Review Focus

Failure modes the spec implies but does not spell out, most likely first. Each has a test in the task named in brackets.

1. A SOCKS request for an IPv6 literal (or any destination the tunnel cannot carry, such as IPv6 on a v4-only tunnel) must be refused, never sent directly. [Tasks 6, 7]
2. Another program on the PC connecting to the local SOCKS port with wrong or no credentials must be refused. [Task 6]
3. A profile whose endpoint is a hostname would be resolved by the system DNS at bring-up, leaking that you use a VPN. The hostname is resolved once at import (a user action) and the IP stored; bring-up never does DNS. [Task 8]
4. After PC sleep or resume the handshake is stale: the lock must hold, then release on its own when a fresh handshake arrives. [Task 9]
5. Malformed or hostile `.conf` files (BOM, CRLF, duplicate keys, huge file, two peers) and rotation with zero, one or all-failing profiles must give clear errors, not panics or a half-applied state. [Tasks 5, 13, 14]

---

### Task 1: Extract `nyx-app` from `src-tauri`

**Files:**
- Create: `crates/nyx-app/Cargo.toml`, `crates/nyx-app/src/lib.rs`
- Move (git mv): `src-tauri/src/{state,dto,feeds,http,poller,update}.rs` to `crates/nyx-app/src/`
- Move: `src-tauri/tests/{commands,features}.rs` to `crates/nyx-app/tests/`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/{lib,commands,instance,tray}.rs` (imports), `Cargo.toml` (members already glob `crates/*`)

**Interfaces:**
- Produces: crate `nyx_app` with `pub mod {state, dto, feeds, http, poller, update}`; `nyx_app::state::AppState` with the same API as today.

- [ ] **Step 1:** Run `cargo test --workspace` and record the passing test count as the baseline.
- [ ] **Step 2:** Create `nyx-app` (deps: `nyx-core`, `nyx-engine`, `serde`, `serde_json`, `chrono`, `ureq`, `sha2`, `regex`, `quick-xml`, `urlencoding`; dev: `tempfile`, `nyx-engine` with `test-support`). Move the six modules and the two AppState-only test files; change `crate::` paths. `src-tauri` keeps `assoc`, `commands`, `instance`, `tray` and imports from `nyx_app`; remove the moved deps from `src-tauri/Cargo.toml` except `tauri*`, `winreg`, `windows-sys`.
- [ ] **Step 3:** Run `cargo test --workspace`. Expected: same passing count as the baseline, no new warnings. Run `cargo build -p nyx` to confirm the app still builds.
- [ ] **Step 4:** Commit `refactor: extract nyx-app from src-tauri`.

### Task 2: `nyx-daemon` headless binary with a loopback control port

**Files:**
- Create: `crates/nyx-daemon/Cargo.toml`, `src/main.rs`, `src/ctl.rs`, `tests/ctl.rs`

**Interfaces:**
- Consumes: `nyx_app::state::AppState::{new, tick, probe_proxy, get_settings, set_settings, add_magnet, add_torrent, list_torrents, rss_run, check_update, search, status}`.
- Produces: binary `nyx-daemon`. `nyx-daemon run` reads env `NYX_DATA_DIR`, `NYX_LISTEN` (default `0.0.0.0:6881`), `NYX_NO_DISCOVERY`; ticks once a second; serves the control port `127.0.0.1:7777`. `nyx-daemon ctl '<json>'` sends one request and prints the reply. Protocol: one JSON object per line, request `{"cmd": "...", ...args}`, reply `{"ok": <value>}` or `{"err": "<message>"}`. Commands (later tasks add more): `status`, `list`, `get_settings`, `set_settings` (`{"settings": {...}}`), `add_magnet` (`uri`, `save_path`), `add_torrent` (`path`, `save_path`), `rss_run`, `check_update`, `search` (`query`), `shutdown`.
- Function: `pub fn handle(state: &AppState, line: &str) -> String` in `ctl.rs` (pure request-to-reply, so it is testable without sockets).

- [ ] **Step 1:** Write `tests/ctl.rs`: `get_settings_roundtrips_through_set_settings`, `unknown_command_returns_err`, `malformed_json_returns_err_not_panic`, `add_magnet_with_relative_save_path_returns_err` (assert `err` contains "full path"). Build the state with `AppState::new(tmp, "127.0.0.1:0".parse().unwrap(), false)`.
- [ ] **Step 2:** Run `cargo test -p nyx-daemon`. Expected: FAIL (crate has no `handle`).
- [ ] **Step 3:** Implement `handle`, the TCP control listener bound to `127.0.0.1:7777` only, the tick loop, background proxy probe and RSS threads (same cadence as `spawn_background` in `src-tauri/src/lib.rs`), and the `ctl` client subcommand.
- [ ] **Step 4:** Run `cargo test -p nyx-daemon`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(daemon): headless nyx-daemon with loopback control port`.

### Task 3: Linux build of the engine and the daemon image

**Files:**
- Modify: `crates/nyx-engine/build.rs`
- Create: `lab/images/nyx-daemon.Dockerfile`, `lab/.dockerignore`

**Interfaces:**
- Produces: Docker image `nyx-lab/daemon` containing `/usr/local/bin/nyx-daemon`.

- [ ] **Step 1:** Make `build.rs` branch on `CARGO_CFG_TARGET_OS`: the MSVC flags (`/EHsc`, `/bigobj`), `WIN32_LEAN_AND_MEAN` and the Windows system libs stay Windows-only; on Linux use `-std=c++17`, link `pthread` and `dl`, and the compile definitions listed in the vcpkg x64-linux libtorrent `LibtorrentRasterbarTargets.cmake` (copy the comment's rule: they must match the prebuilt library). Windows output must not change.
- [ ] **Step 2:** Write the Dockerfile: a Debian build stage that bootstraps vcpkg with this repo's `vcpkg.json` (same pinned libtorrent 2.0.11, triplet `x64-linux`), builds `cargo build --release -p nyx-daemon`, then runs `cargo test -p nyx-daemon` as a build step; the final stage is `debian:stable-slim` with only the binary.
- [ ] **Step 3:** Run `docker build -f lab/images/nyx-daemon.Dockerfile -t nyx-lab/daemon .` then `docker run --rm nyx-lab/daemon nyx-daemon --version`. Expected: build succeeds (first build is long), prints `nyx-daemon 1.0.0`.
- [ ] **Step 4:** Run `cargo test --workspace` on Windows. Expected: still green.
- [ ] **Step 5:** Commit `build: Linux build of the engine, daemon Docker image`.

### Task 4: Network settings types

**Files:**
- Create: `crates/nyx-core/src/network.rs`
- Modify: `crates/nyx-core/src/{lib,settings}.rs`

**Interfaces:**
- Produces in `nyx_core`: `Profile { id: String, name: String, group: String, sealed_conf: String, endpoint: String, endpoint_ip: String }`; `RotationSettings { enabled: bool, pool: Vec<String>, min_minutes: u32, max_minutes: u32 }` (default `false, [], 5, 10`); `NetworkSettings { profiles: Vec<Profile>, active_profile: Option<String>, rotation: RotationSettings }`; `Settings.network: NetworkSettings`. `endpoint` is the host:port for display; `endpoint_ip` is the resolved IP:port used at runtime.

- [ ] **Step 1:** Add tests in `settings.rs`: `old_settings_file_without_network_section_loads_with_defaults` (file `{"max_active_seeds": 9}` gives `network.rotation.min_minutes == 5` and `active_profile == None`), `network_settings_roundtrip` (profile and rotation survive save and load).
- [ ] **Step 2:** Run `cargo test -p nyx-core`. Expected: FAIL (no `network` field).
- [ ] **Step 3:** Implement the types with `#[serde(default)]`, derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`, and add the field to `Settings` and its `Default`.
- [ ] **Step 4:** Run `cargo test -p nyx-core`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): network settings types`.

### Task 5: WireGuard `.conf` parser

**Files:**
- Create: `crates/nyx-tunnel/Cargo.toml`, `src/lib.rs`, `src/conf.rs`

**Interfaces:**
- Produces: `nyx_tunnel::conf::{WgConfig, Endpoint, ConfError, parse_conf}`. `pub struct WgConfig { pub private_key: [u8; 32], pub addresses: Vec<(IpAddr, u8)>, pub dns: Vec<IpAddr>, pub peer_public_key: [u8; 32], pub preshared_key: Option<[u8; 32]>, pub endpoint: Endpoint, pub keepalive: Option<u16> }`; `pub struct Endpoint { pub host: String, pub port: u16 }`; `pub fn parse_conf(text: &str) -> Result<WgConfig, ConfError>`; `ConfError` implements `Display` with a human message per case. Maximum input 64 KiB.

- [ ] **Step 1:** Write tests with a valid sample (Mullvad-style) and: `accepts_bom_crlf_and_comments`; `rejects_two_peers` ("only one peer"); `rejects_missing_endpoint`; `rejects_short_or_non_base64_key`; `rejects_allowed_ips_not_full_route` (`10.0.0.0/8` only); `accepts_allowed_ips_with_both_default_routes`; `rejects_duplicate_private_key`; `rejects_input_over_64_kib`; `error_never_contains_the_private_key` (assert the `Display` text lacks the key string for every error case above).
- [ ] **Step 2:** Run `cargo test -p nyx-tunnel conf`. Expected: FAIL.
- [ ] **Step 3:** Implement a small hand-written INI parser (no INI crate) plus `base64` decoding.
- [ ] **Step 4:** Run `cargo test -p nyx-tunnel conf`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(tunnel): WireGuard conf parser`.

### Task 6: SOCKS5 server over a pluggable backend

**Files:**
- Create: `crates/nyx-tunnel/src/socks.rs`

**Interfaces:**
- Produces: `pub enum Target { Addr(SocketAddr), Domain(String, u16) }`; `pub trait Stream: Read + Write + Send { fn try_clone_box(&self) -> io::Result<Box<dyn Stream>>; }`; `pub trait UdpPort: Send { fn send_to(&self, payload: &[u8], to: &Target) -> io::Result<()>; fn recv(&self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<(usize, SocketAddr)>>; }`; `pub trait Backend: Send + Sync + 'static { fn connect_tcp(&self, t: &Target) -> io::Result<Box<dyn Stream>>; fn udp_open(&self) -> io::Result<Box<dyn UdpPort>>; }`; `pub struct Credentials { pub username: String, pub password: String }` (random per instance); `pub struct SocksServer`; `SocksServer::start(backend: Arc<dyn Backend>) -> io::Result<SocksServer>`, `.addr() -> SocketAddr`, `.credentials() -> Credentials`; dropping it stops the listener.
- Behaviour: RFC 1928 CONNECT and UDP ASSOCIATE, RFC 1929 password auth only (method `0x02`; "no auth" is refused). Backend error maps to reply `0x01` general failure, or `0x03` network unreachable when the backend says the tunnel is down (`io::ErrorKind::NetworkUnreachable`). IPv6 targets are passed to the backend like any other; the backend decides.

- [ ] **Step 1:** Write tests against a `LoopbackBackend` (connects to loopback servers; test-only): `connect_by_ipv4_relays_bytes`; `connect_by_domain_passes_the_name_to_the_backend` (a recording backend asserts it received `Domain("echo.test", 7)` and that nothing was resolved locally); `wrong_password_is_refused`; `no_auth_method_is_refused` (client offers only `0x00`, server replies `0xFF`); `backend_error_gives_general_failure_reply`; `network_unreachable_backend_gives_reply_3`; `udp_associate_relays_a_datagram_both_ways`; `ipv6_target_is_forwarded_to_backend_which_may_refuse` (recording backend refuses, client sees failure).
- [ ] **Step 2:** Run `cargo test -p nyx-tunnel socks`. Expected: FAIL.
- [ ] **Step 3:** Implement with `std::net` threads (one per connection), a 10 s handshake timeout and a cap of 512 concurrent connections.
- [ ] **Step 4:** Run `cargo test -p nyx-tunnel socks`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(tunnel): SOCKS5 server with password auth and pluggable backend`.

### Task 7: Userspace WireGuard tunnel and in-process test peer

**Files:**
- Create: `crates/nyx-tunnel/src/tunnel.rs`, `src/testpeer.rs` (behind feature `test-support`), `tests/tunnel.rs`
- Modify: `crates/nyx-tunnel/{Cargo.toml,src/lib.rs}`

**Interfaces:**
- Consumes: `conf::WgConfig`, `socks::{Backend, SocksServer, Credentials}`.
- Produces: `pub struct Tunnel`; `Tunnel::up(cfg: &WgConfig, endpoint: SocketAddr) -> Result<Tunnel, TunnelError>` (binds the SOCKS server, starts one worker thread that owns the `boringtun` `Tunn`, the UDP socket to `endpoint` and the `smoltcp` interface); `Tunnel::blocked() -> io::Result<Tunnel>` (SOCKS server whose backend always answers network-unreachable; used when no usable profile exists); `.socks_addr() -> SocketAddr`; `.credentials() -> Credentials`; `.stats() -> TunnelStats { handshake_age: Option<Duration>, rx_bytes: u64, tx_bytes: u64 }`; drop shuts everything down. Before the first handshake the backend answers network-unreachable. DNS names are resolved by a `smoltcp` DNS socket using `cfg.dns` through the tunnel; with no DNS configured, domain targets fail. `testpeer::TestPeer::start() -> TestPeer` with `.endpoint() -> SocketAddr`, `.client_config() -> WgConfig`, `.set_paused(bool)` (drops all packets), `.dns_queries() -> Vec<String>`; inside the peer, `10.0.0.1:7` is a TCP echo, UDP `10.0.0.1:7` echoes, and DNS answers every A query with `10.0.0.1` and records the name.

- [ ] **Step 1:** Write `tests/tunnel.rs`: `tcp_echo_through_socks_and_tunnel`; `domain_name_is_resolved_inside_the_tunnel` (connect to `echo.test:7` via SOCKS; assert `dns_queries()` contains `echo.test`); `udp_associate_echo_through_tunnel`; `socks_refuses_before_first_handshake` (peer paused, reply code 3); `ipv6_target_is_refused_on_v4_only_tunnel`; `stats_report_handshake_age_and_bytes`; `blocked_tunnel_refuses_everything`; `shutdown_closes_the_socks_port`. Add `#[ignore] throughput_through_tunnel_on_loopback` that moves 200 MB through the echo peer and asserts at least 10 MB/s (the spec's minimum-speed risk); print the measured rate.
- [ ] **Step 2:** Run `cargo test -p nyx-tunnel --features test-support --test tunnel`. Expected: FAIL.
- [ ] **Step 3:** Implement `Tunnel` and `TestPeer` (the peer is a second `Tunn` with its own `smoltcp` stack; share the device adapter code between them in `tunnel.rs`).
- [ ] **Step 4:** Run the same command, then `cargo test -p nyx-tunnel --features test-support --test tunnel -- --ignored`. Expected: all pass; if the throughput test is under 10 MB/s, stop and report the number instead of lowering the bar.
- [ ] **Step 5:** Commit `feat(tunnel): userspace WireGuard tunnel with local SOCKS5`.

### Task 8: Profile storage, sealing and import

**Files:**
- Create: `crates/nyx-app/src/{secret,profiles}.rs`
- Modify: `crates/nyx-app/Cargo.toml` (`nyx-tunnel`; `windows-sys` with `Win32_Security_Cryptography` under `[target.'cfg(windows)'.dependencies]`), `src/lib.rs`, `src/dto.rs`, `src/state.rs`

**Interfaces:**
- Produces: `secret::seal(plain: &str) -> String` and `secret::open(sealed: &str) -> Result<String, String>` (DPAPI user scope on Windows, tagged `dpapi:`; `plain:` elsewhere); `ProfileDto { id, name, group, endpoint, active }` (no key material); on `AppState`: `import_profile(&self, path: &str, name: &str, group: &str) -> Result<ProfileDto, String>`, `list_profiles(&self) -> Vec<ProfileDto>`, `rename_profile(&self, id: &str, name: &str, group: &str) -> Result<(), String>`, `delete_profile(&self, id: &str) -> Result<(), String>` (refuses the active profile or one in the rotation pool). `profiles::resolve_endpoint(host: &str, port: u16) -> Result<SocketAddr, String>` does the only DNS lookup, at import.

- [ ] **Step 1:** Write tests: `seal_then_open_roundtrips` and `sealed_text_does_not_contain_the_plain_key`; `import_stores_sealed_conf_and_resolves_endpoint_once` (endpoint `127.0.0.1:51820`; stored `endpoint_ip` equals it); `import_rejects_unresolvable_hostname_with_message`; `import_rejects_file_over_64_kib_before_reading_it_all`; `profile_dto_json_never_contains_the_private_key`; `settings_json_on_disk_does_not_contain_the_private_key`; `delete_refuses_active_profile`.
- [ ] **Step 2:** Run `cargo test -p nyx-app profiles secret`. Expected: FAIL.
- [ ] **Step 3:** Implement; ids are random 16 hex characters; save through `set_settings` so validation and persistence stay in one place; extend `validate_settings` with: active profile and every pool id must exist, `min_minutes >= 1`, `min_minutes <= max_minutes`, rotation enabled needs at least 2 pool entries.
- [ ] **Step 4:** Run `cargo test -p nyx-app`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(app): sealed WireGuard profiles and import`.

### Task 9: Network lock and tunnel lifecycle in `AppState`

**Files:**
- Create: `crates/nyx-app/src/netlock.rs`
- Modify: `crates/nyx-app/src/{state,dto,lib}.rs`, `crates/nyx-app/Cargo.toml`, `crates/nyx-app/tests/features.rs`

**Interfaces:**
- Consumes: `Tunnel::{up, blocked, socks_addr, credentials, stats}`, profile types.
- Produces: `netlock::Route { Starting, Up, Stale, Down }` (Serialize); `netlock::STALE_AFTER: Duration = 180 s`; `netlock::START_GRACE: Duration = 30 s`; `pub fn evaluate(handshake_age: Option<Duration>, since_start: Duration) -> Route` (`Some(age)` is `Up` when `age <= STALE_AFTER` else `Stale`; `None` is `Starting` before `START_GRACE`, `Down` after). `NetworkStatusDto { route: Route, profile: Option<String>, handshake_age_secs: Option<u64>, rx_bytes: u64, tx_bytes: u64, exit_ip: Option<String> }`; `AppState::network_status(&self) -> NetworkStatusDto`; `AppState::test_profile(&self, id: &str) -> Result<NetworkStatusDto, String>` (temporary tunnel, waits up to 15 s for a handshake, fetches `https://api.ipify.org` through it for `exit_ip`; user-initiated only, never automatic). `StatusDto` gains `route: String` and `profile_name: Option<String>`.
- Behaviour: with an active profile, `AppState::new` starts the tunnel (or `Tunnel::blocked()` when the profile cannot be opened or brought up) before `Engine::new`; the engine proxy is the tunnel's SOCKS address and credentials and `interface` is `127.0.0.1`. `apply_settings` restarts the tunnel when `active_profile` changes. The kill switch condition becomes `kill_switch && (profile active || proxy set) && !route_ok`, where `route_ok` is `Route::Up` for a profile and the existing probe for a manual proxy; a profile takes precedence over the manual proxy. `http_agent()` uses the tunnel SOCKS settings, and returns `Err("the network lock is holding: no VPN tunnel")` when the route is not `Up` and the lock is on.

- [ ] **Step 1:** Write unit tests in `netlock.rs`: `fresh_handshake_is_up`, `handshake_older_than_180s_is_stale`, `exactly_180s_is_still_up`, `no_handshake_is_starting_then_down_after_grace`. Write integration tests (in `tests/features.rs`, using `TestPeer`): `profile_active_points_engine_at_tunnel_socks`; `cold_start_holds_everything_until_first_handshake` (peer paused at start: after a tick, `status().killed` is true and `http` calls return the lock error; unpause the peer: within 10 s `killed` is false); `peer_dying_holds_torrents_then_recovers` (also covers the sleep and resume case by pausing the peer for longer than the test's shortened stale threshold, injected through a `#[cfg(test)]` constructor argument; no real 180 s wait); `invalid_active_profile_starts_blocked_not_direct` (corrupt sealed conf: `route` is not `Up`, no direct socket opened); `manual_proxy_still_works_without_a_profile`; `profile_wins_over_manual_proxy`.
- [ ] **Step 2:** Run `cargo test -p nyx-app --features nyx-tunnel/test-support`. Expected: FAIL.
- [ ] **Step 3:** Implement. Replace `proxy_ok`/`probe_proxy` use with a `route_ok()` helper that covers both cases, keep `probe_proxy` for the manual proxy, and have the poller call a new `refresh_route()` each tick.
- [ ] **Step 4:** Run `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(app): network lock and tunnel lifecycle`.

### Task 10: Lab skeleton with the egress and identity checks

**Files:**
- Create: `lab/docker-compose.yml`, `lab/run.ps1`, `lab/run.sh`, `lab/images/{wg-exit,tracker,observer,dns,canary,gateway,runner}/...`, `lab/runner/{scenarios.py,analyzer.py,report.py,torrentgen.py}`, `lab/README.md`, `.gitignore` entry for `lab/out/`

**Interfaces:**
- Lab topology: network `real` (`10.77.0.0/24`, `internal: true`) holds `client-a` (and later `client-b`), the gateway `10.77.0.1`; network `internet` (`10.88.0.0/24`) holds the gateway `10.88.0.1` (NAT masquerade, the client's "ISP identity"), `wg-exit-a` `.10`, `wg-exit-b` `.11`, `tracker` `.20`, `seeder` (a `nyx-daemon`, direct) `.21`, `observer` `.22`, `dns` `.23`, `canary` `.24`. Each client has a `tcpdump` sidecar (`network_mode: service:client-a`) writing `/out/client-a.pcap`. All servers write JSON lines to `/out/<name>.log`. The runner container drives clients with `docker exec <client> nyx-daemon ctl '<json>'`.
- `analyzer.py`: `check_egress(pcap, allowed_endpoints) -> list[Finding]` (only UDP to an allowed WireGuard endpoint may leave the client; allowed housekeeping: ARP and ICMPv6 neighbour discovery to the link-local gateway; everything else, including any IPv6, mDNS, LLMNR, DHCP, ICMP and DNS, is a finding); `check_identity(server_logs) -> list[Finding]` (a finding for any log entry whose source IP is the gateway `10.88.0.1`; sources must be `wg-exit-a` or `wg-exit-b`); `Finding(check, scenario, detail)`. `run.ps1` and `run.sh` run `docker compose up --build --abort-on-container-exit --exit-code-from runner`; the runner exits non-zero if any finding exists and writes `lab/out/report.md`.
- Scenarios in this task: `cold_start` (client starts with an active profile whose exit is reachable; log shows no packet before the first WireGuard handshake except WireGuard to the endpoint), `steady_download` (client downloads a 20 MB generated torrent from the seeder via tracker, completes, hash verified).

- [ ] **Step 1:** Write `analyzer.py` unit tests (`lab/runner/test_analyzer.py`, run with `python -m unittest` in the runner image) using small synthetic pcaps built with `dpkt`: `plain_tcp_to_internet_is_a_finding`, `wireguard_udp_to_allowed_endpoint_is_clean`, `wireguard_udp_to_other_ip_is_a_finding`, `ipv6_packet_is_a_finding`, `arp_is_clean`, `mdns_is_a_finding`, `server_log_with_gateway_source_is_a_finding`.
- [ ] **Step 2:** Run the analyzer tests in the runner image. Expected: FAIL, then implement `analyzer.py`.
- [ ] **Step 3:** Build the compose stack and the two scenarios. The runner generates WireGuard keys with `wg` (in the `wg-exit` image: `wireguard-go` plus `wireguard-tools`, `NET_ADMIN`, `/dev/net/tun`, masquerade to `internet`), writes client `.conf` files, imports them with `import_profile`, activates profile A through `set_settings`.
- [ ] **Step 4:** Run `lab\run.ps1`. Expected: both scenarios complete; the report lists findings (the first run is the baseline and may fail). Copy `lab/out/report.md` to `lab/FINDINGS.md` as the baseline record.
- [ ] **Step 5:** Commit `feat(lab): Docker leak lab skeleton with egress and identity checks`.

### Task 11: Remaining scenarios and the kill-timing check

**Files:**
- Modify: `lab/runner/{scenarios.py,analyzer.py}`, `lab/docker-compose.yml`; create `lab/images/bait/...`

**Interfaces:**
- Produces: `analyzer.check_kill_timing(pcap, kill_time, allowed_endpoints, held_within_s=10) -> list[Finding]` (after `kill_time`, no non-allowlisted bytes leave, and `list` shows every torrent `queued` within `held_within_s`). New ctl command `network_status` returning `NetworkStatusDto`.
- Scenarios: `tunnel_killed_midtransfer` (runner drops WireGuard packets on `wg-exit-a` with `iptables -I INPUT -p udp --dport 51820 -j DROP`, waits, checks timing, restores, checks recovery), `hostname_trackers` (tracker announced by hostname via the lab DNS: the DNS log must show queries only from `wg-exit-a`/`wg-exit-b` sources), `metadata_requests` (RSS, search and update check against `canary`: every request source is an exit), `bait` (tracker and peers return LAN-style addresses, IP literals, IPv6 addresses and an HTTP redirect to `canary` by IP), `discovery_attempts` (client runs with default discovery settings on: LSD, UPnP, NAT-PMP, DHT must produce zero packets), `shutdown_and_crash` (clean `shutdown`, and `docker kill` mid-transfer; pcap shows nothing non-allowlisted during or after), `ipv6_icmp_broadcast` (the capture sees none).

- [ ] **Step 1:** Add analyzer unit tests: `traffic_after_kill_is_a_finding`, `quiet_after_kill_is_clean`, `wireguard_keepalive_attempts_to_endpoint_are_clean`.
- [ ] **Step 2:** Run them. Expected: FAIL; then implement `check_kill_timing` and `network_status` in the daemon (add a ctl test in `crates/nyx-daemon/tests/ctl.rs`).
- [ ] **Step 3:** Implement the scenarios and run `lab\run.ps1`.
- [ ] **Step 4:** Update `lab/FINDINGS.md` with the new baseline: each FAIL with the scenario, the packet or log line, and the suspected cause.
- [ ] **Step 5:** Commit `feat(lab): full scenario set and kill timing check`.

### Task 12: Triage and patch the first findings

**Files:** determined by `lab/FINDINGS.md`; expected candidates are `crates/nyx-engine/cpp/shim.cpp` (`fill_proxy`, `Session` ctor), `crates/nyx-app/src/state.rs`, `crates/nyx-app/src/{http,update}.rs`.

**Procedure and exit criterion:** the lab exits 0 for every scenario in Tasks 10 and 11. For each finding in `lab/FINDINGS.md`, in order: (a) confirm the scenario already fails for exactly that finding (that scenario is the failing test); (b) fix the root cause once in shared code; (c) run `lab\run.ps1` and confirm that finding is gone and no new one appeared; (d) run `cargo test --workspace`; (e) commit as `fix(privacy): <finding>` with the finding id in the body; (f) tick it off in `FINDINGS.md`. Candidates to check first: libtorrent `anonymous_mode` (hides client version and listen port; not set in `fill_proxy` today), tracker `User-Agent` and `peer_id` prefix revealing the client and version, any `ip=` or `ipv6=` tracker parameter, `ureq` redirects to IP literals, and a first-tick window before the kill switch applies to restored torrents.

- [ ] **Step 1:** Run `lab\run.ps1` and list the findings.
- [ ] **Step 2:** Work the procedure above for each finding until the lab exits 0.
- [ ] **Step 3:** Run `cargo test --workspace` and `lab\run.ps1`. Expected: both green.

### Task 13: Rotation scheduler (pure logic)

**Files:**
- Create: `crates/nyx-app/src/rotation.rs`; add `rand` to `crates/nyx-app/Cargo.toml`

**Interfaces:**
- Produces: `pub fn next_interval(min_minutes: u32, max_minutes: u32, rng: &mut impl Rng) -> Duration` (uniform in the closed range, in whole seconds); `pub fn pick_next(pool: &[String], current: &str, rng: &mut impl Rng) -> Option<String>` (random, never `current`; `None` when no other entry); `pub struct Rotator { pub fn new(now: Instant, settings: &RotationSettings, rng: &mut impl Rng) -> Rotator; pub fn due(&self, now: Instant) -> bool; pub fn reset(&mut self, now: Instant, settings: &RotationSettings, rng: &mut impl Rng); pub fn backoff(&mut self, now: Instant) }` where `backoff` schedules a retry in 30 seconds. The caller passes `Instant`, so tests need no real waiting.

- [ ] **Step 1:** Write tests with a seeded `StdRng`: `interval_stays_within_min_and_max_over_1000_draws`; `min_equal_max_gives_exact_interval`; `pick_next_never_returns_current_over_1000_draws`; `pick_next_with_single_entry_is_none`; `pick_next_with_current_not_in_pool_still_picks`; `due_is_false_before_and_true_after_interval`; `backoff_retries_after_30_seconds`; `pick_next_eventually_visits_every_other_entry`.
- [ ] **Step 2:** Run `cargo test -p nyx-app rotation`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p nyx-app rotation`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(app): rotation scheduling logic`.

### Task 14: Make-before-break rotation and the swap-gap check

**Files:**
- Modify: `crates/nyx-app/src/state.rs`, `crates/nyx-daemon/src/{main,ctl}.rs` (command `rotate_now`), `lab/runner/{scenarios.py,analyzer.py}`

**Interfaces:**
- Produces: `AppState::rotate_now(&self) -> Result<String, String>` (returns the new profile id). `AppState::rotation_tick(&self, now: Instant)` (called from `AppState::tick`, so the app and the daemon both get it: runs `rotate_now` when due, backs off on failure). `NetworkStatusDto` gains `next_rotation_secs: Option<u64>`. `analyzer.check_swap_gap(pcap, swap_window, endpoints: [a, b]) -> list[Finding]`.
- Behaviour (spec 4.3): bring up the new tunnel and wait up to 15 s for its handshake; on success repoint the engine with `set_proxy`, store the new tunnel as current, drop the old; on failure drop nothing that would reopen a direct path: the old tunnel is dropped, the lock holds torrents (route `Down`), and rotation retries the next profile after the backoff. With rotation disabled or fewer than 2 pool entries `rotate_now` returns an error and changes nothing.

- [ ] **Step 1:** Write tests (with `TestPeer` x2): `rotate_now_switches_to_other_profile_and_engine_follows`; `rotate_now_with_unreachable_target_holds_torrents_and_never_goes_direct`; `rotation_tick_does_nothing_before_due`; `rotation_tick_rotates_after_interval_and_resets_timer`; `rotate_now_errors_with_one_profile`; `all_profiles_failing_does_not_panic_and_keeps_lock_on`. Add analyzer test `swap_packets_to_third_ip_is_a_finding`.
- [ ] **Step 2:** Run `cargo test -p nyx-app rotation_state`. Expected: FAIL.
- [ ] **Step 3:** Implement; add scenario `rotation_swap` (lab uses `rotate_now` three times during a download, then waits for one timer-driven rotation with a 60 to 60 second interval) and run `lab\run.ps1`; fix any new finding using the Task 12 procedure.
- [ ] **Step 4:** Run `cargo test --workspace` and `lab\run.ps1`. Expected: green.
- [ ] **Step 5:** Commit `feat(app): make-before-break rotation`.

### Task 15: Cross-rotation linkability check and fix

**Files:**
- Modify: `lab/runner/{analyzer.py,scenarios.py}`, `crates/nyx-engine/{src/lib.rs,cpp/shim.cpp,cpp/shim.hpp,src/ffi.rs}`, `crates/nyx-app/src/state.rs`

**Interfaces:**
- Produces: `analyzer.check_linkability(tracker_log, observer_log, rotation_marks) -> list[Finding]` (a finding when the peer_id, the tracker `key`, or the client version string seen before a rotation equals the one seen after it, or when the version string reveals the Nyx or libtorrent version). Engine API `Engine::rotate_identity(&self) -> Result<(), EngineError>`, called by `rotate_now` after the engine is repointed.

- [ ] **Step 1:** Add analyzer tests `same_peer_id_across_rotation_is_a_finding`, `same_tracker_key_across_rotation_is_a_finding`, `different_identity_is_clean`. Run: FAIL, implement `check_linkability`.
- [ ] **Step 2:** Run `lab\run.ps1` (scenario `rotation_swap` now records identities). Expected: FAIL on peer_id and tracker key (libtorrent fixes both for the life of a session).
- [ ] **Step 3:** Implement `rotate_identity`: first try updating `peer_fingerprint` and check whether the shim sees a new peer_id; if libtorrent does not regenerate it, rebuild the `lt::session` from `session_state()` and the saved resume data (the same path the existing restart test `crates/nyx-engine/tests/restart.rs` exercises), keeping torrent state. Add an engine test `rotate_identity_keeps_torrents_and_changes_peer_id` using the existing test helpers.
- [ ] **Step 4:** Run `cargo test --workspace` and `lab\run.ps1`. Expected: green, FINDINGS.md updated.
- [ ] **Step 5:** Commit `fix(privacy): new peer_id and tracker key on every rotation`.

### Task 16: Network page and Tauri commands

**Files:**
- Modify: `src-tauri/{src/commands.rs,src/lib.rs,build.rs,capabilities/default.json}`, `ui/src/lib/{types.ts,api.ts,mock.ts}`, `ui/src/components/{SettingsPage.svelte,TitleBar.svelte,Banners.svelte}`
- Create: `ui/src/components/NetworkPage.svelte`, `ui/src/components/NetworkPage.test.ts`

**Interfaces:**
- Consumes: `AppState::{import_profile, list_profiles, rename_profile, delete_profile, network_status, test_profile, rotate_now, get_settings, set_settings}`.
- Produces Tauri commands (register in `generate_handler!`, `build.rs` `COMMANDS` and the capability file): `list_profiles`, `import_profile(path, name, group)`, `rename_profile(id, name, group)`, `delete_profile(id)`, `test_profile(id)`, `get_network_status`, `rotate_now`. TypeScript types `Profile`, `NetworkStatus` (mirroring the DTOs) and `Settings.network` (profiles omitted; `active_profile`, `rotation`). The file picker uses the existing dialog plugin and passes the chosen path.
- UI (spec section 6): profile list with import, rename, delete and Test; status card (profile, handshake age, bytes, exit IP, lock state); lock toggle bound to `kill_switch` with a warning when off; rotation section (on/off, pool checkboxes, minimum and maximum minutes, the private-tracker warning, "Rotate now"); a title-bar route indicator that is red while `killed`.

- [ ] **Step 1:** Write `NetworkPage.test.ts` (Vitest, pattern of `SettingsPage.test.ts`): `lists_profiles_without_any_key_material`; `import_button_calls_import_profile_with_the_chosen_path`; `lock_toggle_off_shows_warning`; `rotation_needs_two_selected_profiles_to_enable`; `private_tracker_warning_shown_when_rotation_on`; `title_bar_indicator_is_red_when_killed`.
- [ ] **Step 2:** Run `npm test` in `ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement the page, commands, mocks and registration. Replace `proxy_down`-only wording in `Banners.svelte` so a held lock names the route state.
- [ ] **Step 4:** Run `npm test` and `npm run check` in `ui/`, `cargo test --workspace`, then the Playwright smoke test (`e2e`, off-screen as the existing runs do). Expected: all green.
- [ ] **Step 5:** Commit `feat(ui): Network settings page and route indicator`.

### Task 17: Documentation and final verification

**Files:**
- Modify: `README.md`, `docs/superpowers/specs/2026-10-27-network-privacy-design.md` (status line), `lab/README.md`

- [ ] **Step 1:** Add a README section "Network privacy": how to import a WireGuard config, the lock, rotation and its tracker warning, and what the lab proves and does not prove (the spec's "Explicitly not claimed" list, verbatim in meaning).
- [ ] **Step 2:** Set the spec status to "Plan 1 implemented; Tor and pktmon smoke test pending".
- [ ] **Step 3:** Run the whole verification: `cargo test --workspace`, `cargo test -p nyx-tunnel --features test-support -- --ignored`, `npm test` and `npm run check` in `ui/`, `e2e` smoke, `lab\run.ps1`. Expected: everything green and `lab/out/report.md` lists zero findings.
- [ ] **Step 4:** Commit `docs: network privacy`. Stop here: pushing, merging and releasing are separate steps.
