# Nyx leak lab: baseline findings (Task 11, full scenario set)

Baseline run: 2026-10-27 15:14 UTC (run 3 of Task 11, `lab\run.ps1`). The full report of that
run is `lab/out/report.md` (git-ignored); the parts that matter are copied here.

Result: **0 scenarios failed (infrastructure), 2 findings (leaks), positive control complete.**
Exit code 1, as expected while findings are open: Task 12 fixes the app; this task records them
and does not touch app code.

## Scenarios

| Scenario | Ran | Detail |
|---|---|---|
| cold_start | yes | route up after 0.7s (wg-exit-a), 2 WireGuard packets before; profile B imported; daemon without NET_ADMIN |
| steady_download | yes | 20 MB in 3s, SHA-256 verified, 30 s seeding dwell |
| hostname_trackers | yes | 1 tracker.lab lookup, from 10.88.0.10 (exit A) only; announce received |
| metadata_requests | yes | canary saw rss 1, search 1, update check (:443) 1 requests, all from 10.88.0.10 |
| bait | yes | bait served; the redirect to the canary by IP was followed 2x, from 10.88.0.10; no bait peer dialled outside the tunnel |
| discovery_attempts | yes | 90 s dwell, discovery at its default (on) |
| tunnel_killed_midtransfer | yes | killed at 4%; held after 14.9 s (stale 20 s + 10 s allowed); 78 WireGuard packets dropped; recovered 0.2 s after the drop was lifted |
| shutdown_and_crash | yes | clean shutdown: back up in 0.8 s, 5 torrents restored; SIGKILL at 3%: back up in 0.7 s, download finished, SHA-256 verified |
| ipv6_icmp_broadcast | yes | client has fd77::a and an IPv6 default route; 8 MB downloaded, 20 s dwell |
| positive_control | yes | all 6 deliberate leaks detected (TCP tracker, DNS, UDP exit A :9999, UDP over IPv6 to fd77::1, tracker and dns identity) |

## Findings (leaks) for Task 12

### F1. The network lock releases while the tunnel is still cut (kill_timing)

- Scenario: `tunnel_killed_midtransfer`, window "killed" (exit A drops every WireGuard packet
  from the client: `iptables -I INPUT -p udp --dport 51820 -j DROP`).
- Evidence (run 3), samples taken once a second after the torrents were held, as (seconds after
  the kill, `status.killed`, `network_status.route`, `handshake_age_secs`):
  `(15, True, up, 0), (16, False, up, 1), (17, False, up, 2), (18, False, up, 3), (20, False, up, 4),
  (21, False, up, 0), (22, False, up, 1), (23, False, up, 2), (24, False, up, 3)`.
  Report line: `kill_timing | tunnel_killed_midtransfer (killed) | lock released while the tunnel was
  still cut: 8 of 9 samples, first 16.3 s after the kill (route up, handshake age 1 s)`.
  Run 2 showed the same ("route then up" right after the hold).
- Meaning: the lock holds once (at 14.9 s, inside the 30 s deadline), then the route reads `up`
  again with a handshake age that restarts at 0 about every 5 s, although no packet reaches the
  exit and no handshake can complete. The kill switch releases the torrents into a dead tunnel.
  Nothing leaked on the wire (the capture holds only WireGuard to exit A in this window), but the
  lock and the UI status are wrong, and any route that depends on "up" is trusted while dead.
- Suspected cause: `nyx-tunnel/src/tunnel.rs` `Wire::publish` copies boringtun's
  `tunn.stats()` handshake age into `handshake_at`. That age appears to follow handshake
  *initiations* (boringtun retries every 5 s, REKEY_TIMEOUT; `Wire::tick` also sends
  initiations at the rekey age), not completed handshakes. `NetTiming::evaluate`
  (`nyx-app/src/netlock.rs`) then sees a fresh handshake. Fix in Task 12: record the time of the
  last *completed* handshake (a response received / session established) and evaluate staleness
  against that.

### F2. ICMP port-unreachable to the VPN endpoint while the daemon is down (egress)

- Scenario: `shutdown_and_crash` (the clean `shutdown` and the SIGKILL, each followed by a few
  seconds down before the restart).
- Evidence (run 3): `egress | shutdown_and_crash | ICMP type 3 -> 10.88.0.10: 13 packets`. The
  run 1 capture shows them as ICMP type 3 code 3 (port unreachable) to 10.88.0.10, in bursts during
  the down periods (runs 1, 2 and 3: 12, 12 and 13 packets).
- Meaning: after the daemon exits, its WireGuard UDP socket is gone, but exit A keeps sending to
  the client's old port (tunnelled peer traffic and retransmissions still in flight). The client
  kernel answers each datagram with ICMP port unreachable. It only goes to the VPN endpoint, which
  already knows the client's address. So it reveals nothing new, but it is traffic outside the
  allowlist, and it tells the exit (and any on-path observer) that the client went away abruptly.
- Suspected cause: no tunnel teardown on exit. `nyx-daemon` `shutdown` calls `process::exit(0)`
  right after `save_state`, and a crash cannot clean up. Options for Task 12: close peer
  connections and the tunnel before exiting (fewer packets in flight). Or accept the finding
  as an OS behaviour outside the app's control and allowlist ICMP to the endpoint in the lab, with
  a written reason. A crash will always produce some of it.

## Analysis

**The lab can see leaks (positive control).** It is unchanged from Task 10, plus an IPv6 leak
(UDP to the lab gateway fd77::1). The tracker and dns identity signatures are now pinned to the
control's own request (path `/positive-control`, query `control.lab`). A gateway entry with any
other path or name in that window counts as a real finding.

**Capture complete and live.** The sniffer is stopped before the analysis, so the pcap is
complete. Every window of 30 s or more had WireGuard to exit A and a packet in its last 30 s.

**Clean, by scenario:**
- *Hostname trackers*: `tracker.lab` was resolved once, through the tunnel (the dns saw exit A),
  never by the system resolver.
- *Metadata*: the RSS poll, the search and the update check (api.github.com, pointed at the
  canary's :443) all came from exit A. The update check fails in the lab (no TLS), as intended.
- *Bait*: no packet to 10.77.0.x, 192.168.x, fd77::/2001:db8:: or the canary left the client
  outside the tunnel, and the HTTP redirect to the canary by IP was followed through exit A.
- *Discovery*: no DHT, LSD, UPnP or NAT-PMP packet in any window (about 12 minutes of run, with
  discovery on). Limit: libtorrent's LSD announces every 5 minutes, and the app has no lab-only
  way to shorten that without an app change, so the longest practical dwell (90 s) is used. LSD
  announces at torrent start fall in the earlier windows (7 torrents were added) and none showed.
  A periodic LSD announce is covered only if one falls inside the run.
- *Kill timing*: held within the deadline (14.9 s; stale threshold 20 s via the lab-only
  `NYX_LAB_STALE_AFTER_SECS`), recovered in 0.2 s. Apart from F1, no packet left outside the
  allowlist while the tunnel was cut.
- *Restarts*: both restarts were clean cold starts (only WireGuard to exit A before the route was
  up); restored torrents were held until then.
- *IPv6/ICMP/broadcast*: with IPv6 on, the client sends only kernel link housekeeping (MLD
  reports, router solicitations, neighbour discovery to ff02::). That traffic is allowed, like
  ARP. Nothing else used IPv6, and no broadcast or mDNS was seen.

**Lab bugs found and fixed in Task 11 (not findings):**
- IPv6 housekeeping was classified too narrowly: MLD reports (behind a hop-by-hop header) and
  ND sent from the ULA address were reported as leaks in run 1. They are now allowed when the
  destination is link-local or link-scope multicast (ff02::/16); an ICMPv6 echo there is still a
  finding.
- The crash test expected the torrent added just before the SIGKILL to survive. The daemon saves
  resume data every 60 s, so it was gone after the restart. That is not a leak; the lab notes it
  and adds the torrent again.

**Observations for later tasks (not findings of these checks):**
- *HTTP User-Agent*: HTTP tracker announces send `User-Agent: Nyx/0.1`, which names the app.
- *Fingerprint*: peer_id prefix `-LT20B0-` (libtorrent 2.0.11); `key` is stable within a torrent's
  session; UDP announces carry `port=1 ip=0.0.0.0`.
- *Lost torrent on crash*: a torrent added less than 60 s before a crash is not restored.
- *Observer blind to MSE*: every connection to the observer was encrypted (Task 15).
- *UDP ASSOCIATE port 0* (carry-over): libtorrent announces port 0, so the relay adopts the first
  local sender.
