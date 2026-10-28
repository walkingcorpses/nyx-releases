# Nyx leak lab

Docker Compose lab that runs the headless Nyx (`nyx-daemon`) behind a simulated home router with
WireGuard VPN exits, and checks that nothing leaves the client outside the tunnel (design spec
section 7).

## Run

```powershell
lab\run.ps1        # Windows (Docker Desktop)
```
```sh
lab/run.sh         # Linux / macOS / Git Bash
```

The script builds `nyx-lab/daemon` from the repo root (the first build compiles libtorrent and
takes a long time; later builds are cached), removes any leftover lab (`docker compose down -v`,
project `nyx-lab`), clears `lab/out/`, starts the lab detached and follows the runner's output,
then tears the lab down. The exit code is the runner's: non-zero if any check found a leak, a
scenario could not run, or the positive control was not fully detected. A full run takes about
13 minutes.

Dev runs of a subset: set `LAB_ONLY=<scenario>[,<scenario>]` before the script (cold_start and
the positive control always run), e.g. `$env:LAB_ONLY='tunnel_killed_midtransfer'; lab\run.ps1`.
An unknown name fails the run at once (the error lists the valid names), and a subset run's
verdict reads "PARTIAL RUN (LAB_ONLY=...)", never a clean lab.

The lab runs detached (not `up --abort-on-container-exit`) because scenarios stop and restart
the client daemon and stop the capture sidecar before the analysis.

Output in `lab/out/` (git-ignored, copied even when the runner crashes): `report.md`,
`client-a.pcap` (every packet leaving the client), `tracker.log`, `observer.log`, `dns.log`,
`canary.log` (JSON lines). `lab/FINDINGS.md` is the committed baseline record.

## Topology

| Network | Host | Address |
|---|---|---|
| real 10.77.0.0/24 + fd77::/64 | gateway | .1 / fd77::1 |
| | client-a (+ tcpdump sidecar) | .10 / fd77::a |
| | the Docker host (bridge address) | .254 / fd77::fe |
| internet 10.88.0.0/24 (internal) | gateway (NAT: the client's real identity) | .1 |
| | wg-exit-a / wg-exit-b (wireguard-go) | .10 / .11 |
| | tracker (HTTP :8000, UDP :6969) | .20 |
| | seeder (nyx-daemon, direct) | .21 |
| | observer (passive peer :6881) | .22 |
| | dns (:53) | .23 |
| | canary (HTTP :80, TCP logger :443) | .24 |

Note: the Docker host is reachable from the `real` LAN on 10.77.0.254 / fd77::fe (Docker's own
bridge address; the network cannot be `internal`, see docker-compose.yml). Both default routes of
the client point at the lab gateway instead, and the bridge does not masquerade, so a leak never
reaches the real internet; it would still be seen in the capture.

The runner (no network; drives containers over the Docker socket) generates WireGuard keys, the
exit configs and the client profiles, and seven test torrents. The client starts with VPN profile
A already active (settings pre-seeded), kill switch on, discovery at its default (on), queue
limits raised so the queue never holds a lab torrent. The client's entry script sets its routes,
then drops NET_ADMIN (`setpriv`) before the daemon runs, and supervises the daemon so a scenario
can stop or kill it and start it again on the same data dir.

`NYX_LAB_STALE_AFTER_SECS=20` (client only) is a LAB-ONLY override read by `nyx-daemon` alone: the
network lock calls the tunnel stale after 20 s without a handshake instead of the production
180 s, so the kill-timing check does not wait three minutes.

`NYX_LAB_ROTATE_SECS=60` (client only) is a LAB-ONLY override read by `nyx-daemon` alone: while
rotation is on (only in the rotation scenarios) it rotates every 60 s instead of the settings'
whole minutes, so `rotation_swap` sees one timer-driven swap.

## Checks

Every scenario has a time window; the windows are contiguous, so each packet and log line is
checked once. The capture sidecar is stopped before the analysis, so the pcap is complete.

- **egress**: in `client-a.pcap` (Ethernet only; another link type is an error), only UDP to the
  active profile's WireGuard endpoint may appear, plus ARP and IPv6 neighbour discovery from a
  link-local address. Everything else is a finding.
- **identity** (allowlist): a lab server may only log the exits (10.88.0.10/.11) or the lab
  servers' own addresses (10.88.0.20-.24). The gateway 10.88.0.1 (the client's real address), the
  raw client address or anything else is a finding. A log line that cannot be parsed is a finding
  too (`logs`).
- **capture live**: a window of 30 s or more only counts if the capture holds WireGuard to exit A
  in it and a packet in its last 30 s; otherwise the scenario fails (infrastructure).
- **kill timing**: see `tunnel_killed_midtransfer`.
- **swap gap**: inside each profile swap (from the `rotate_now` call, or the last reading on the
  old exit for a timer swap, to 2 s past the swap) only WireGuard to the old or the new exit may
  leave, plus the housekeeping above. In the rotation scenarios' windows the egress check allows
  both exits; the port-unreachable allowance stays pinned to each exit's own WireGuard flow.

- **linkability** (spec 7.3 check 3, `identity_rotation`): a finding when the same peer_id
  (tracker announce or peer handshake), tracker `key` or non-empty extension `v` is seen in two
  sessions (either side of a rotation mark) or from two exits (which also catches an old identity
  slipping out through the new tunnel before the mark); when a peer_id prefix, `v` or tracker
  User-Agent names Nyx or libtorrent (`-LT....-`, "libtorrent", "nyx"); and when a listen port is
  announced: the extension handshake `p` or the announce `port`, other than libtorrent's
  placeholder 1 (or 0). A proxied client cannot accept connections, so a real port there is its
  true listen port or a stable per-session token. libtorrent sends `port=1` and no `p` while
  proxied; libtorrent's anonymous-mode User-Agent `curl/7.81.0` is not counted (see FINDINGS).

The report separates scenarios that could not run (infrastructure failures) from findings (leaks).

## Scenarios

- `cold_start`: from the first captured packet until the tunnel is up.
- `steady_download`: 20 MB from the seeder via the tracker (IP literals), SHA-256 verified, 30 s
  seeding dwell.
- `hostname_trackers`: a torrent whose trackers are `tracker.lab`; the lab dns must see the
  lookup, and only from an exit.
- `metadata_requests`: RSS feed and search provider set to `canary.lab`, update check (the lab dns
  points api.github.com at the canary's :443 logger); `rss_run`, `search`, `check_update`. Every
  canary request must come from an exit.
- `bait`: the tracker returns home-LAN, private-IP and IPv6 peers, and an HTTP redirect to the
  canary by IP. Nothing may leave outside the tunnel.
- `discovery_attempts`: 90 s dwell with discovery on. libtorrent's LSD announces every 5 min and
  the app has no lab-only way to shorten that, so only LSD's announce at torrent start is covered.
- `tunnel_killed_midtransfer`: exit A drops the client's WireGuard (`iptables -I INPUT -p udp
  --dport 51820 -j DROP`) mid-download (seeder capped at 256 KiB/s). After the kill only WireGuard
  to exit A may leave; every torrent must read held (`queued`, `status.killed`) within the stale
  threshold + 10 s and stay held until the drop is lifted; after that the route must be up, the
  torrent released AND rx bytes or the torrent's progress must grow past their values at the
  restore, within 60 s (state flags alone do not prove recovery). The cut is one-way: exit A
  still reaches the client with handshake initiations and with data on its old keypair (finding
  F1: neither may count as a live tunnel). The lock is sampled once a second from the kill; the
  notes list (s after kill, killed, route, handshake age, rx bytes).
- `tunnel_cut_both_ways`: the same, with exit A also dropping its own WireGuard to the client
  (`iptables -I OUTPUT -p udp --sport 51820 -j DROP`).
- `tunnel_cut_long`: the one-way cut held for 130 s, longer than wireguard-go's ~90 s of
  handshake attempts plus the stale threshold: held all through, then the same recovery check.
- `shutdown_and_crash`: a clean `shutdown` and a restart; then SIGKILL of the daemon mid-download
  (a crash: the container and its capture stay up) and a restart. Each restart must be a clean
  cold start; the crashed download must finish.
- `ipv6_icmp_broadcast`: IPv6 is on in the LAN (the client has fd77::a and an IPv6 default
  route). A download must produce no IPv6, ICMP, broadcast or mDNS.
- `rotation_swap`: rotation on with exits A and B in the pool; during a 40 MB download (seeder
  capped at 128 KiB/s) `rotate_now` three times (A, B, A, B), then one timer-driven swap. Each
  swap must reach the other exit and the download's progress must grow again within 45 s; then
  back to A with rotation off, the download finished and SHA-256 verified.
- `rotation_failure_holds`: exit B drops the client's WireGuard, so `rotate_now` from A to B must
  fail (no contact in 15 s). For 15 s after it the lock must hold (route down, torrents `queued`;
  a `rotation_lock` finding otherwise) with nothing direct; then the drop is lifted and
  `rotate_now` back to A must get the download moving within 60 s.
- `identity_rotation`: rotation on (A and B in the pool), encryption set to "Prefer" for the
  scenario, a 40 MB download (seeder capped at 128 KiB/s); `rotate_now` twice (A, B, A). Each
  session (before, between and after the swaps) must reach the tracker and send the observer a
  plaintext handshake within 90 s, or the scenario fails (not exercised). Then the linkability
  check runs over the client's tracker and observer entries (from the exits only; the seeder
  announces too and never rotates), with each `rotate_now` reply as a mark. The download must
  still finish, SHA-256 verified. Why "Prefer": the observer has no MSE/PE, and under "Forced"
  every connection to it is encrypted and unreadable. MSE only wraps the BitTorrent handshake and
  the extension handshake; their content (peer_id, reserved bits, `v`, `p`, `yourip`, `reqq`,
  `m`) is the same under Forced, so what the observer reads is what any MSE peer learns. libtorrent
  tries MSE first and falls back to plaintext on the next connect.
- `positive_control` (last): deliberate leaks from client-a's namespace: TCP to the tracker
  (`/positive-control`), a DNS query for `control.lab` to the lab dns, UDP to exit A on port 9999,
  UDP over IPv6 to fd77::1. All four egress findings and the tracker/dns identity findings
  (pinned to that path and query name) must be detected, or the run fails with "lab blind".
  These expected findings are reported in their own section and do not count as leaks; any other
  finding in that window still does.

Analyzer and report unit tests: `docker build -f lab/images/runner/Dockerfile -t nyx-lab/runner lab`
then `docker run --rm nyx-lab/runner python -m unittest test_analyzer`.
