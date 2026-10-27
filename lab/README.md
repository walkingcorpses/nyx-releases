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
takes a long time; later builds are cached), clears `lab/out/`, runs every scenario and tears the
lab down (`docker compose down -v`, project `nyx-lab`). The exit code is non-zero if any check
found a leak or a scenario could not run.

Output in `lab/out/` (git-ignored): `report.md`, `client-a.pcap` (every packet leaving the
client), `tracker.log`, `observer.log`, `dns.log`, `canary.log` (JSON lines). `lab/FINDINGS.md`
is the committed baseline record.

## Topology

| Network | Host | Address |
|---|---|---|
| real 10.77.0.0/24 | gateway | .1 |
| | client-a (+ tcpdump sidecar) | .10 |
| internet 10.88.0.0/24 (internal) | gateway (NAT: the client's real identity) | .1 |
| | wg-exit-a / wg-exit-b (wireguard-go) | .10 / .11 |
| | tracker (HTTP :8000, UDP :6969) | .20 |
| | seeder (nyx-daemon, direct) | .21 |
| | observer (passive peer :6881) | .22 |
| | dns (:53) | .23 |
| | canary (HTTP :80) | .24 |

The runner (no network; drives containers over the Docker socket) generates WireGuard keys, the
exit configs and the client profiles, and a 20 MB test torrent. The client starts with VPN
profile A already active (settings pre-seeded), kill switch on, discovery at its default.

## Checks

- **egress**: in `client-a.pcap`, only UDP to the active profile's WireGuard endpoint may appear
  (plus ARP and IPv6 neighbour discovery). Everything else is a finding.
- **identity**: no server may log the source `10.88.0.1` (the gateway, i.e. the client's real
  address); traffic through the tunnel arrives from the exit's address.

## Scenarios

- `cold_start`: from the first captured packet until the tunnel is up.
- `steady_download`: the client downloads the torrent from the seeder via the tracker; the file's
  SHA-256 is verified; then 60 s of seeding. It fails unless the capture was live all through:
  WireGuard to exit A in the window and a packet in its last 30 s (keepalive is 25 s).
- `positive_control` (last): deliberate leaks from client-a's namespace: TCP to the tracker, a DNS
  query to the lab dns, UDP to exit A on port 9999. All three egress findings and the tracker/dns
  identity findings must be detected, or the run fails with "lab blind". These expected findings
  are reported in their own section and do not count as leaks; any other finding in that window
  still does.

Analyzer unit tests: `docker build -f lab/images/runner/Dockerfile -t nyx-lab/runner lab` then
`docker run --rm nyx-lab/runner python -m unittest test_analyzer`.
