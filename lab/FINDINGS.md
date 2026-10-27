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
| tunnel_killed_midtransfer | yes | killed at 4%; held after 14.9 s (stale 20 s + 10 s allowed); 78 WireGuard packets dropped; recovery not proven in this run (baseline run, see F1 fix note) |
| shutdown_and_crash | yes | clean shutdown: back up in 0.8 s, 5 torrents restored; SIGKILL at 3%: back up in 0.7 s, download finished, SHA-256 verified |
| ipv6_icmp_broadcast | yes | client has fd77::a and an IPv6 default route; 8 MB downloaded, 20 s dwell |
| positive_control | yes | all 6 deliberate leaks detected (TCP tracker, DNS, UDP exit A :9999, UDP over IPv6 to fd77::1, tracker and dns identity) |

## Findings (leaks) for Task 12

### F1. A half-finished handshake counts as a live tunnel: the lock releases while still cut (kill_timing)

- [x] **FIXED in Task 12a** (commits e249b5f, c5110cf; lab scenarios 03107b9). Proven by the full
  lab run of 2026-10-27 16:03 UTC: no kill_timing finding; the only finding left is F2.
  - Fix: `TunnelStats.handshake_age` (nyx-tunnel) is now the time since CONFIRMED contact, i.e.
    a handshake response to OUR initiation that boringtun accepted (`completes_our_handshake`).
    Received initiations (type 1), cookie replies (type 3) and inbound transport data (type 4)
    never count. The client re-key also counts from that record, not from boringtun's
    `time_since_last_handshake`.
  - Why inbound data does not count either (lab evidence, one-way cut, data counted as contact):
    the age reset at 1, 2, 5, 7, 14 and 27 s after the kill, each time rx grew by exactly one
    1420-byte packet. These were the seeder's TCP retransmits (exponential backoff), sent by exit A
    on its still-valid keypair while the client's packets were dropped.
  - Full run results: `tunnel_killed_midtransfer` (one-way) held after 15.3 s, stayed held for
    the whole cut, recovered 6.0 s after the restore. `tunnel_cut_both_ways` held after 12.9 s,
    recovered after 2.5 s. `tunnel_cut_long` (one-way, 130 s) held after 7.0 s (age 13 s at the
    kill), stayed held for all 130 s (past wireguard-go's ~90 s of attempts), recovered after
    3.7 s. Every recovery showed rx and progress growth. The subset run before it gave the same
    picture: ages grew from 0 to 129 s without one reset, while rx still grew from exit A's
    retransmits.
  - Tests: nyx-tunnel `half_handshake_does_not_count_as_contact`,
    `initiation_alone_does_not_reset_age`, `data_alone_does_not_reset_age`,
    `completed_rekey_counts_as_contact` (TestPeer `set_inbound_cut`); nyx-app
    `one_way_cut_holds_torrents_and_keeps_them_held`.

Original finding (Task 11 baseline):

- Scenario: `tunnel_killed_midtransfer`, window "killed". The lab cut is ONE-WAY: exit A drops
  the client's WireGuard packets (`iptables -I INPUT -p udp --dport 51820 -j DROP` on wg-exit-a),
  but exit A can still send to the client.
- Evidence (run 3), samples taken once a second after the torrents were held, as (seconds after
  the kill, `status.killed`, `network_status.route`, `handshake_age_secs`):
  `(15, True, up, 0), (16, False, up, 1), (17, False, up, 2), (18, False, up, 3), (20, False, up, 4),
  (21, False, up, 0), (22, False, up, 1), (23, False, up, 2), (24, False, up, 3)`.
  Report line: `kill_timing | tunnel_killed_midtransfer (killed) | lock released while the tunnel was
  still cut: 8 of 9 samples, first 16.3 s after the kill (route up, handshake age 1 s)`.
  The kill-only rerun after the review gave the same picture: `(15, True, stale, 20), (16, False, up, 0),
  ... (21, False, up, 0), ...`. Run 2 showed it too ("route then up" right after the hold).
- Meaning: the lock holds once, inside the 30 s deadline. Then the route reads `up` again, with a
  handshake age that restarts at 0 about every 5 s, although no packet from the client reaches
  exit A. The kill switch releases the torrents into a dead tunnel. Nothing leaked on the wire
  (only WireGuard to exit A in this window), but the lock and the UI status are wrong.
- Mechanism (from boringtun 0.7.1's source, checked in the Task 11 review):
  - `noise/timers.rs:314` `time_since_last_handshake` is now minus `TimeSessionEstablished`
    whenever the current session slot is filled.
  - `noise/mod.rs:318-336` `handle_handshake_init`: when the client RECEIVES an initiation,
    boringtun stores a new responder session and calls `timer_tick_session_established` before
    any confirmation from the peer. The client's own initiations do not reset the age.
  - In the lab, exit A (wireguard-go) still has data queued for the client. After about 15 s
    without hearing from it, exit A starts initiating and retries every ~5 s (RekeyTimeout).
  - Each inbound initiation resets the client's handshake age to 0, while the client's response
    is dropped by the cut. This matches the evidence: age 0 first at ~15 s, then a reset every
    ~5 s, and only WireGuard on the wire.
  - So a half-handshake (initiation received, never confirmed) is treated as a live tunnel. A
    real asymmetric path failure would trigger it for as long as wireguard-go keeps trying to
    reach the client (up to its ~90 s handshake attempt window).
- Why the hold fired at 14.9 s, before the 20 s threshold: the threshold counts from the last
  handshake, not from the kill. The rerun recorded a handshake age of 5 s at the kill and a hold
  14.8 s after it, which is 20 s - 5 s plus at most the 1 s poll tick.
- Suggested fix for Task 12:
  - Judge liveness on authenticated inbound traffic after a CONFIRMED session: the rx counter
    growing, or data or a keepalive received on the current session, or a completed handshake
    that the client initiated.
  - Do NOT rely on "session established" alone (boringtun's `time_since_last_handshake` /
    `stats()` age, which `nyx-tunnel/src/tunnel.rs` `Wire::publish` copies into
    `handshake_at` for `NetTiming::evaluate`).
- Lab follow-ups for Task 12:
  - Add a both-directions cut variant (drop the client's packets in and out on exit A).
  - Add a cut that lasts longer than 90 s plus the stale threshold, so the lock is checked
    after wireguard-go has stopped initiating.

### F2. ICMP port-unreachable to the VPN endpoint while the daemon is down (egress)

- [x] **RESOLVED BY NARROW ALLOWLIST in Task 12b** (lab analyzer, not app code).
  - Why not an app fix: the packets are the client KERNEL's ICMP destination unreachable (type 3,
    code 3, port unreachable) answers to datagrams that exit A (wireguard-go) keeps sending to the
    client's WireGuard UDP port after the daemon closed that socket (clean shutdown or SIGKILL).
    Linux rate-limits them to about 1/s. Datagrams in flight always arrive after the socket is
    closed, and a crash cannot tear anything down, so no app-side teardown removes them.
  - Why it is acceptable: they go only to the VPN endpoint IP, which is the WireGuard peer and
    already knows the client's real address. They reveal nothing new.
  - The allowance (`analyzer._wireguard_port_unreachable`, used by `check_egress`) accepts an
    ICMP packet only if ALL hold: type 3 and code 3; destination is an allowed endpoint IP; the
    quoted datagram is IPv4 UDP from that endpoint IP and its WireGuard port (51820) to the
    client's own address (the ICMP packet's source). Any other ICMP type or code, any other
    destination (the gateway, other hosts), a quoted TCP header, a wrong quoted port or address,
    and a quote too short to check stay findings. There is no blanket ICMP allowance.
  - Tests: `test_port_unreachable_answer_to_endpoint_wireguard_is_clean`,
    `test_other_icmp_shapes_stay_findings` (type 3 code 1, echo, destination not an endpoint,
    quoted source port not 51820, quoted TCP, quoted destination not the client, quoted source
    not the endpoint, truncated quote, ICMP to the gateway). Checked on the real Task 12a capture:
    21 ICMP packets, all of this shape. The positive control has no ICMP sender (bash
    /dev/tcp and /dev/udp only), so the negative cases are covered by the unit tests.

Original finding (Task 11 baseline):

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
  `NYX_LAB_STALE_AFTER_SECS`). Apart from F1, no packet left outside the allowlist while the
  tunnel was cut.
- *Recovery*: the baseline's "recovered after 0.2 s" proved nothing. Because of F1 the lock had
  already released before the drop was lifted, so "route up, not killed, not queued" was true at
  once. Since the review, recovery needs positive evidence: `rx_bytes` (ctl `network_status`) or
  the torrent's progress must grow past its value at the restore, within 60 s.
  Kill-only rerun: recovered 2.5 s after the drop was lifted, with rx 836060 -> 1313557 and
  progress 4% -> 6%.
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
