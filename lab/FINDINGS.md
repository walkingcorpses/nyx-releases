# Nyx leak lab: baseline findings (copy of lab/out/report.md + analysis)

Run: 2026-10-27 11:54:31 UTC

## Scenarios

| Scenario | Ran | Detail |
|---|---|---|
| cold_start | yes | route up after 0.1s (wg-exit-a), 2 WireGuard packets before; profile B imported |
| steady_download | yes | 20 MB in 3s, SHA-256 verified |

## Findings (0)

None.

## Notes

- observer saw 10.88.0.10: peer_id=None v=None p=None reserved=None encrypted=True error=- (x13)
- tracker udp announce from 10.88.0.10: peer_id=-LT20B0-7d3O6np~fAKU key=ada386a2 port=1 ip=0.0.0.0 event=2 UA=-
- tracker udp announce from 10.88.0.10: peer_id=-LT20B0-7d3O6np~fAKU key=ada386a2 port=1 ip=0.0.0.0 event=1 UA=-
- SOCKS UDP ASSOCIATE (from NYX_LAB_DEBUG): nyx-lab: socks UDP ASSOCIATE announced port 0

## Analysis (baseline, Task 10)

Runs 3 to 5 of 2026-10-27 gave the same outcome: both scenarios ran, **0 findings**.

**Egress is clean.** Each run's client capture (only packets leaving client-a's eth0) held about
1,450 to 2,030 packets: all UDP to 10.88.0.10:51820 (exit A) plus 4 to 6 ARP frames. That covers
the cold start, the download and a 60 s seeding dwell with discovery left at its default (ON). So,
with a profile active, DHT, LSD, UPnP and NAT-PMP sent nothing on the LAN in these windows, and no
DNS lookup left the client (the trackers are IP literals, so the profile's DNS was not needed).

**Identity is clean.** Every tracker and observer entry came from 10.88.0.10 (exit A), never from
10.88.0.1 (the gateway, i.e. the client's real address). The lab can see that source: a manual
probe from a container on the real network, routed through the gateway, was logged by the tracker
as `src 10.88.0.1` while the lab was being built.

**Observations for later tasks (not findings of this task's checks):**

- *Fingerprint.* The peer_id prefix `-LT20B0-` announces libtorrent 2.0.11 rather than a neutral
  identity, and the tracker `key` stays the same across one session's announces (linkability
  across a rotation is still to be measured with exit B). Over UDP the client announces `port=1`,
  `ip=0.0.0.0`: libtorrent hides its real listen port behind the proxy.
- *Observer is blind to encrypted peers.* All 13 connections to the observer used MSE/PE
  encryption, so it could not read the handshake (peer_id, reserved bits, `v`, `yourip`). The
  observer needs an MSE responder (or a run with encryption set to "prefer") before the peer-side
  fingerprint check can work.
- *UDP ASSOCIATE (carry-over).* libtorrent's UDP tracker traffic goes through the tunnel's SOCKS5
  UDP ASSOCIATE and libtorrent announces **port 0** in the request. With port 0 the relay
  (socks.rs `udp_loop`) adopts the first local sender from the control connection's IP, so a local
  process that reaches the loopback relay port first could claim the association. This is the
  deferred SOCKS minor; it is not reachable from the network and did not leak in the lab.
- *Short window.* The download takes about 3 s; the 60 s dwell adds steady state. Long-period
  traffic (LSD every 5 min, DHT refresh) is not covered by this baseline.
