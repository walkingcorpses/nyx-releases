# Nyx: Network Privacy (VPN, Rotation, Tor, Leak Lab) Design Spec

Date: 2026-10-27
Status: Approved. Plan 1 implemented (VPN tunnel, network lock, rotation, identity rotation, UI, leak lab). Tor metadata routing and the `pktmon` smoke test are still to do.

## 1. Purpose

Let the owner hook up VPNs from the Nyx UI, stop all transfers whenever the VPN is not connected, rotate between VPNs or servers over time, and send torrent metadata through Tor. Then prove, with a simulated network of Nyx clients and servers, that Nyx does not reveal the owner's real IP address or a linkable identity, and use that lab to find and patch leaks.

### Success criteria

- A WireGuard `.conf` file imported in the UI brings up a tunnel; peer and tracker traffic leaves only through it.
- When the tunnel is down or stale, no torrent traffic and no HTTP request leaves the machine (the "network lock"), including at cold start before the tunnel first comes up.
- Optional rotation switches profile at a random interval (default 5 to 10 minutes) with no direct-connection window during the swap.
- With Tor on, RSS, search, `.torrent` downloads over HTTP and the update check go through Tor, never directly.
- The leak lab runs from one command, fails on any leak, and every leak it has found is fixed and kept as a regression scenario.

### Explicitly not claimed

"Totally private" is not achievable against everything. Swarm peers always see the exit IP. The lab proves that the real IP and identifying metadata do not leave the tunnel under the simulated conditions. It does not cover: the VPN provider logging, traffic-timing correlation by swarm peers, real Tor network behaviour, and Windows-specific behaviour (see section 7).

### Out of scope

OpenVPN and system-wide VPN control; kernel drivers (wireguard-nt, Wintun) and admin rights; Tor for peer traffic; port forwarding through the tunnel (can be added later by relaying a provider-forwarded port to the loopback listener); per-torrent routing rules; password-locked app.

## 2. Decisions taken during brainstorming

| Question | Decision |
|---|---|
| How to hook up a VPN | Built-in userspace WireGuard, import `.conf`. No admin, no driver, only Nyx traffic is tunneled. |
| Tor scope | Metadata only (RSS, search, `.torrent` downloads over HTTP, update check). Peer data goes through the tunnel. |
| Lab style | Headless Linux build (`nyx-daemon`) of the same engine and app code, run in Docker. Plus a small `pktmon` smoke test on the real `nyx.exe`. |

## 3. Architecture

### 3.1 Crates

| Crate | Status | Responsibility |
|---|---|---|
| `nyx-tunnel` | new | Userspace WireGuard client (`boringtun`) over a userspace TCP/IP stack (`smoltcp`), with a local SOCKS5 server on `127.0.0.1:<random port>` protected by a per-session username and password. DNS is resolved inside the tunnel. Reports status: last handshake time, bytes in and out, exit endpoint. |
| `nyx-torroute` | new | Embedded Tor client (`arti`) exposing a local SOCKS5 port. Reports bootstrap progress. No separate tor binary is shipped. |
| `nyx-app` | extracted from `src-tauri` | `state.rs`, poller, feeds, http, update: everything that is not Tauri or Windows shell code. Owns the network lock and rotation. |
| `nyx-daemon` | new | Headless binary over `nyx-app`, used by the lab (Linux) and usable for tests. |
| `src-tauri` | slimmed | Tauri shell, tray, file association, commands that call `nyx-app`. |
| `nyx-core` | extended | Settings gain a `network` section (profiles, rotation, Tor flag, lock flag); config parsing and validation. |
| `lab/` | new | Docker Compose lab, scenarios and the analyzer (section 7). |

The extraction of `nyx-app` is a prerequisite: `state.rs` has no Tauri imports today, so it moves without redesign, but `windows-sys` and `winreg` uses must stay behind `cfg(windows)` in `src-tauri` so `nyx-app` builds on Linux.

### 3.2 Profiles

A profile holds: name, group label (usually the provider), WireGuard interface (private key, addresses, DNS), peer (public key, optional pre-shared key, endpoint, allowed IPs). Import accepts the standard `.conf` format and rejects, with a specific message: wrong key length or encoding, missing endpoint, `AllowedIPs` that is not a full route (`0.0.0.0/0`, `::/0`), more than one peer. Private keys are encrypted with Windows DPAPI before being written to `settings.json`; on Linux (lab only) they are stored as given. A key is never logged, never sent to the UI after import, and never appears in lab captures.

## 4. Routing and the network lock

### 4.1 Routes

Every outbound path has exactly one allowed route:

- Peers and tracker announces (engine): libtorrent has a single proxy, so announces and scrapes stay on the tunnel and cannot use Tor. The only proxy is the tunnel's local SOCKS5 port. In addition `outgoing_interfaces` is bound to `127.0.0.1` so a direct connection from a bug fails instead of leaking.
- RSS, search, `.torrent` downloads over HTTP, update check (HTTP agent): the Tor SOCKS5 port when Tor is on, otherwise the tunnel.
- No other code path opens a socket to a non-loopback address. The lab enforces this.

The existing proxy restrictions stay: DHT, uTP, LSD, UPnP and NAT-PMP off, listener on loopback only.

### 4.2 Network lock (kill switch)

Replaces the TCP probe in `probe_proxy` when a profile is active. The tunnel is down when no WireGuard handshake has completed within 180 seconds (a healthy tunnel re-keys about every 120 seconds). While down:

- every running torrent is held with the existing `kill_held` mechanism and new torrents start paused;
- the HTTP agent refuses requests (as `http_agent` does when the proxy is down today);
- everything resumes automatically on a fresh handshake.

The lock is on by default. With the lock on, Nyx starts no network activity until the tunnel's first handshake. The lock also covers Tor: if Tor is enabled and not ready, metadata requests fail; there is no fallback to a direct or tunnel-only route for requests that should use Tor.

### 4.3 Rotation

Opt-in, off by default.

- Pool: any subset of profiles (servers of one provider, or several providers).
- Interval: random between a minimum and maximum, default 5 and 10 minutes. The next profile is random and never the current one.
- Switch is make-before-break: bring up the new tunnel and wait for its handshake, repoint the engine with `set_proxy`, then tear down the old tunnel. The lock stays armed during the swap, so a failed swap holds torrents and does not fall back to the old route or to a direct connection.
- If a profile fails to come up, rotation tries the next one after a short backoff.
- The UI warns that rotation resets peer connections and presents a new IP to trackers each cycle, which some private trackers ban.

## 5. Failure handling

- Tunnel fails to come up or times out: lock engages; the UI names the failed profile.
- Config invalid at startup: Nyx starts with the lock on and no route and says so. It does not fall back to direct.
- Tor not ready: only metadata requests wait or fail. Torrent traffic is unaffected.
- Old `proxy` and `bind_interface` settings remain as an "advanced: manual proxy" option. If a profile is active it wins and the UI says so.

## 6. UI

New page Settings, Network:

- profile list with import, rename, delete, and a Test button (brings the tunnel up once, checks the handshake, shows the exit IP seen through it);
- status card: current profile, handshake age, bytes in and out, exit IP, lock state;
- network lock toggle (on by default, warning when turned off);
- rotation section: on/off, pool checkboxes, minimum and maximum minutes, private-tracker warning;
- Tor toggle with readiness state (for example "bootstrapping 60%", "ready").

The title bar shows the active route and turns red while the lock is holding torrents.

## 7. Leak lab

### 7.1 Layout (`lab/`, Docker Compose)

The client sits on a "real" network with a distinctive address, so any server that sees that address has caught a leak. It reaches the "internet" network only through a NAT gateway.

- Clients: one or more `nyx-daemon` containers, each with a `tcpdump` sidecar sharing its network namespace that records every packet leaving its interface.
- Two WireGuard exit servers (A and B) so rotation can be tested.
- A tracker (HTTP and UDP) logging the source IP and full announce: peer_id, port, any `ip=` value, `key`, User-Agent.
- A seeder and an observer client logging what a peer sees: peer_id, client version string, extension handshake, reported IP.
- A DNS server logging every query and its origin.
- A canary web server standing in for RSS, search and update endpoints, logging headers.
- Bait servers and peers returning LAN addresses, IP literals, IPv6 addresses and HTTP redirects to try to pull Nyx off the tunnel.

### 7.2 Scenarios

Cold start before the tunnel is up; steady download; tunnel killed mid-transfer by dropping its packets; rotation swap; tracker hostname resolution; RSS fetch, search and update check; shutdown and crash; IPv6, ICMP, and broadcast discovery (LSD, mDNS, UPnP).

### 7.3 Checks (pass or fail)

1. Egress allowlist: the only packets leaving the client interface are WireGuard datagrams to the active profile's endpoint. Anything else is a leak.
2. Identity: no server logs the client's real address.
3. Fingerprint and linkability: peer_id, version strings and the tracker `key` are audited, including whether they stay the same across a rotation (expected to be a finding: a tracker could link sessions across IPs).
4. Kill timing: after the tunnel dies, zero non-allowlisted bytes leave and torrents are held within a set number of seconds.
5. Swap gap: during rotation, packets go only to the old or new endpoint.

### 7.4 Output and workflow

One command (`lab/run.ps1`, with a shell equivalent) builds the images, runs every scenario and writes a report plus pcaps to `lab/out/`. Any leak gives a non-zero exit code. Patch loop: each finding is first added as a failing scenario, then fixed, then kept as a regression. A nightly CI run is possible later (GitHub runners have Docker).

### 7.5 Limits

- Windows-only behaviour (adapter binding, firewall, cold start of the real exe) is covered only by a small `pktmon` smoke test on `nyx.exe`.
- Tor in the first version: the lab checks that metadata requests go only to the local Tor SOCKS port. A private Tor test network is a later step.
- The lab uses a simulated WireGuard server; it does not test any real provider.

## 8. Testing in the normal suite

Unit tests: config parsing and validation; the lock state machine (up, stale, down, recovery); rotation scheduling with a fake clock. Integration tests run `nyx-tunnel` against an in-process WireGuard peer. UI tests for the Network page follow the existing `SettingsPage.test.ts` pattern.

## 9. Build order

1. Extract `nyx-app` and add `nyx-daemon` (no behaviour change; existing tests stay green).
2. `nyx-tunnel` and profile import; lock wired to handshake age.
3. Lab skeleton with the egress and identity checks, run against step 2.
4. Rotation, then Network page UI.
5. Patch loop on lab findings.
6. `nyx-torroute` and Tor metadata routing, with its lab check.
7. `pktmon` smoke test on `nyx.exe`.

## 10. Risks

- Userspace TCP/IP (`smoltcp`) throughput may be lower than a kernel tunnel; measure early in step 2 and set a minimum acceptable speed.
- `arti` adds binary size and build time; check the size impact in step 6 before committing to it.
- Without DHT, uTP and inbound peers, swarm health depends on trackers and outgoing connections only (already true in proxy mode today).
- Cross-compiling libtorrent and the engine for the Linux daemon is new; budget time for the build in step 1.
