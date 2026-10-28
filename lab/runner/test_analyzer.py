"""test_analyzer.py - unit tests for the leak lab analyzer, on tiny synthetic pcaps built with dpkt.
Run in the runner image: `python -m unittest test_analyzer`."""
import os
import socket
import tempfile
import unittest

import dpkt

import report
from analyzer import (CONTROL_SIGNATURES, Finding, capture_live, check_egress, check_identity,
                      check_kill_timing, check_linkability, check_recovery, check_swap_gap, count_udp_to, load_logs, recovered,
                      split_control)

CLIENT_MAC = b"\x02\x42\x0a\x4d\x00\x0a"
GW_MAC = b"\x02\x42\x0a\x4d\x00\x01"
CLIENT_IP = "10.77.0.10"
EXIT_A = ("10.88.0.10", 51820)
EXIT_B = ("10.88.0.11", 51820)


def ip4(src, dst, proto, payload):
    """An IPv4 packet carrying `payload` (a dpkt TCP/UDP/ICMP object)."""
    return dpkt.ip.IP(src=socket.inet_aton(src), dst=socket.inet_aton(dst), p=proto, data=payload)


def ip6(src, dst, nxt, payload):
    """An IPv6 packet carrying `payload`."""
    return dpkt.ip6.IP6(src=socket.inet_pton(socket.AF_INET6, src), dst=socket.inet_pton(socket.AF_INET6, dst),
                        nxt=nxt, hlim=255, data=payload, plen=len(bytes(payload)))


def eth(payload, etype=dpkt.ethernet.ETH_TYPE_IP, dst=GW_MAC):
    """An Ethernet frame from the client."""
    return dpkt.ethernet.Ethernet(src=CLIENT_MAC, dst=dst, type=etype, data=payload)


def udp(dst, sport, dport, data=b"x"):
    """An outbound UDP datagram from the client to `dst`."""
    return eth(ip4(CLIENT_IP, dst, dpkt.ip.IP_PROTO_UDP, dpkt.udp.UDP(sport=sport, dport=dport, data=data)))


class Analyzer(unittest.TestCase):
    """check_egress and check_identity on hand-made inputs."""

    def pcap(self, *frames, linktype=dpkt.pcap.DLT_EN10MB):
        """Writes `frames` to a temp pcap (one second apart, from ts 1000) and returns its path."""
        fd, path = tempfile.mkstemp(suffix=".pcap")
        with os.fdopen(fd, "wb") as f:
            w = dpkt.pcap.Writer(f, linktype=linktype)
            for i, fr in enumerate(frames):
                w.writepkt(bytes(fr), ts=1000.0 + i)
        self.addCleanup(os.remove, path)
        return path

    def test_plain_tcp_to_internet_is_a_finding(self):
        """A direct TCP connection (here to the tracker) is a leak."""
        tcp = dpkt.tcp.TCP(sport=40000, dport=8000, flags=dpkt.tcp.TH_SYN)
        f = check_egress(self.pcap(eth(ip4(CLIENT_IP, "10.88.0.20", dpkt.ip.IP_PROTO_TCP, tcp))), {EXIT_A})
        self.assertEqual(len(f), 1)
        self.assertIn("TCP", f[0].detail)
        self.assertIn("10.88.0.20:8000", f[0].detail)

    def test_wireguard_udp_to_allowed_endpoint_is_clean(self):
        """WireGuard datagrams to the active endpoint are the only allowed traffic."""
        self.assertEqual(check_egress(self.pcap(udp("10.88.0.10", 45000, 51820), udp("10.88.0.10", 45000, 51820)), {EXIT_A}), [])

    def test_wireguard_udp_to_other_ip_is_a_finding(self):
        """WireGuard to an endpoint that is not the active profile's is still a leak."""
        f = check_egress(self.pcap(udp("10.88.0.11", 45000, 51820)), {EXIT_A})
        self.assertEqual(len(f), 1)
        self.assertIn("10.88.0.11:51820", f[0].detail)

    def test_ipv6_packet_is_a_finding(self):
        """Any IPv6 traffic except neighbour discovery is a leak."""
        pkt = ip6("fe80::42:aff:fe4d:a", "2001:db8::1", dpkt.ip.IP_PROTO_UDP, dpkt.udp.UDP(sport=1, dport=53, data=b"q"))
        f = check_egress(self.pcap(eth(pkt, dpkt.ethernet.ETH_TYPE_IP6)), {EXIT_A})
        self.assertEqual(len(f), 1)
        self.assertIn("IPv6", f[0].detail)

    def test_ipv6_neighbour_solicitation_is_clean(self):
        """ICMPv6 neighbour solicitation from link-local to a solicited-node address is housekeeping."""
        ns = dpkt.icmp6.ICMP6(type=135, code=0, data=b"\x00" * 20)
        pkt = ip6("fe80::42:aff:fe4d:a", "ff02::1:ff00:1", dpkt.ip.IP_PROTO_ICMP6, ns)
        self.assertEqual(check_egress(self.pcap(eth(pkt, dpkt.ethernet.ETH_TYPE_IP6)), {EXIT_A}), [])

    def test_arp_is_clean(self):
        """ARP for the gateway is housekeeping."""
        arp = dpkt.arp.ARP(sha=CLIENT_MAC, spa=socket.inet_aton(CLIENT_IP), tha=b"\x00" * 6, tpa=socket.inet_aton("10.77.0.1"))
        frame = eth(arp, dpkt.ethernet.ETH_TYPE_ARP, dst=b"\xff" * 6)
        self.assertEqual(check_egress(self.pcap(frame), {EXIT_A}), [])

    def test_mdns_is_a_finding(self):
        """Multicast DNS on the LAN is a leak, and is labelled as mDNS."""
        f = check_egress(self.pcap(udp("224.0.0.251", 5353, 5353)), {EXIT_A})
        self.assertEqual(len(f), 1)
        self.assertIn("mDNS", f[0].detail)

    def test_repeated_flow_is_one_finding(self):
        """Many packets of the same flow give one finding with a packet count."""
        f = check_egress(self.pcap(*[udp("10.88.0.23", 40000 + i, 53) for i in range(5)]), {EXIT_A})
        self.assertEqual(len(f), 1)
        self.assertIn("5 packets", f[0].detail)

    def test_time_window_limits_the_check(self):
        """Only packets inside [since, until] count (scenario windows)."""
        p = self.pcap(udp("10.88.0.10", 1, 51820), udp("10.88.0.23", 1, 53))
        self.assertEqual(check_egress(p, {EXIT_A}, until=1000.5), [])
        self.assertEqual(len(check_egress(p, {EXIT_A}, since=1000.5, scenario="s")), 1)

    def test_count_udp_to_counts_only_that_endpoint(self):
        """count_udp_to sees the WireGuard packets and nothing else."""
        p = self.pcap(udp("10.88.0.10", 1, 51820), udp("10.88.0.11", 1, 51820), udp("10.88.0.10", 1, 51820))
        self.assertEqual(count_udp_to(p, EXIT_A), 2)

    def test_capture_live_fails_on_empty_capture(self):
        """An empty capture (header only) proves nothing: the liveness check must fail."""
        ok, why = capture_live(self.pcap(), EXIT_A, 0, 2000)
        self.assertFalse(ok)
        self.assertIn("no WireGuard", why)

    def test_capture_live_fails_when_capture_stalls_before_window_end(self):
        """WireGuard early in the window but nothing in its last 30 s: the writer stalled."""
        p = self.pcap(udp("10.88.0.10", 1, 51820))  # ts 1000
        ok, why = capture_live(p, EXIT_A, 0, 1100, tail=30)
        self.assertFalse(ok)
        self.assertIn("last 30", why)

    def test_capture_live_passes_with_recent_packet(self):
        """WireGuard in the window and a packet within the tail: live."""
        p = self.pcap(udp("10.88.0.10", 1, 51820), udp("10.88.0.10", 1, 51820))  # ts 1000, 1001
        self.assertEqual(capture_live(p, EXIT_A, 0, 1020, tail=30), (True, ""))

    def test_split_control_excludes_only_expected_signatures_in_control_window(self):
        """Control findings matching the expected signatures are set aside; a different leak in the
        same window, and a matching leak in another window, still count as real."""
        sigs = CONTROL_SIGNATURES
        fs = [Finding("egress", "positive_control", "TCP -> 10.88.0.20:8000 (HTTP tracker): 3 packets, first at 1.0"),
              Finding("egress", "positive_control", "UDP -> 10.88.0.23:53 (DNS): 1 packets, first at 1.0"),
              Finding("egress", "positive_control", "UDP -> 10.88.0.10:9999: 1 packets, first at 1.0"),
              Finding("identity", "positive_control", "tracker saw 10.88.0.1 at 1.000: {'path': '/positive-control'}"),
              Finding("identity", "positive_control", "dns saw 10.88.0.1 at 1.000: {'name': 'control.lab'}"),
              Finding("egress", "positive_control", "IPv6 -> [fd77::1] port 9999: 1 packets, first at 1.0"),
              Finding("egress", "positive_control", "UDP -> 10.88.0.23:5353 (mDNS): 1 packets, first at 1.0"),
              Finding("egress", "steady_download", "TCP -> 10.88.0.20:8000 (HTTP tracker): 1 packets, first at 0.5")]
        real, missing = split_control(fs, "positive_control", sigs)
        self.assertEqual(missing, [])
        self.assertEqual(real, [fs[6], fs[7]])

    def test_split_control_reports_missing_signature(self):
        """A control leak that was not detected is reported as missing (the lab is blind to it)."""
        fs = [Finding("egress", "positive_control", "TCP -> 10.88.0.20:8000 (HTTP tracker): 3 packets, first at 1.0")]
        real, missing = split_control(fs, "positive_control", CONTROL_SIGNATURES)
        self.assertEqual(real, [])
        self.assertEqual(len(missing), len(CONTROL_SIGNATURES) - 1)
        self.assertNotIn("TCP to the tracker", missing)

    def test_server_log_with_gateway_source_is_a_finding(self):
        """A server that logged the gateway (the client's real address) caught a leak."""
        logs = [{"server": "tracker", "ts": 1.0, "src": "10.88.0.1", "proto": "http"}]
        f = check_identity(logs, scenario="steady_download")
        self.assertEqual(f, [Finding("identity", "steady_download", f[0].detail)])
        self.assertIn("tracker", f[0].detail)

    def test_server_log_from_exit_is_clean(self):
        """Entries that came through an exit (or from other lab servers) are fine."""
        logs = [{"server": "tracker", "ts": 1.0, "src": "10.88.0.10"}, {"server": "observer", "ts": 2.0, "src": "10.88.0.11"},
                {"server": "tracker", "ts": 3.0, "src": "10.88.0.21"}]
        self.assertEqual(check_identity(logs), [])

    # ---- Task 11: kill timing, recovery, identity allowlist, log parsing, control, datalink ----

    # Real frames from client-a in lab run 1 (IPv6 on in the LAN): kernel link housekeeping.
    MLD_REPORT = ("3333000000165ed173fc858d86dd6000000000380001fe800000000000005cd173fffefc858dff020000000000000000000000000016"
                  "3a000502000001008f0093000000000204000000ff0200000000000000000001fffc858d04000000ff0200000000000000000001ff00000a")
    NA_FROM_ULA = ("3333000000015ed173fc858d86dd6001ef2500203afffd77000000000000000000000000000aff020000000000000000000000000001"
                   "8800034120000000fd77000000000000000000000000000a02015ed173fc858d")
    NS_FROM_ULA = ("3333ff0000015ed173fc858d86dd6000000000203afffd77000000000000000000000000000aff0200000000000000000001ff000001"
                   "8700264800000000fd77000000000000000000000000000101015ed173fc858d")

    def test_ipv6_link_housekeeping_from_any_client_address_is_clean(self):
        """MLD reports (behind a hop-by-hop header) and neighbour discovery to link-scope
        multicast are the kernel's IPv6 'ARP', also when sent from the client's ULA address."""
        frames = [bytes.fromhex(h) for h in (self.MLD_REPORT, self.NA_FROM_ULA, self.NS_FROM_ULA)]
        self.assertEqual(check_egress(self.pcap(*frames), {EXIT_A}), [])

    def test_icmpv6_echo_to_link_multicast_is_a_finding(self):
        """Only neighbour discovery and MLD are housekeeping: a ping to all-nodes is not."""
        echo = dpkt.icmp6.ICMP6(type=128, code=0, data=b"\x00" * 8)
        pkt = ip6("fd77::a", "ff02::1", dpkt.ip.IP_PROTO_ICMP6, echo)
        self.assertEqual(len(check_egress(self.pcap(eth(pkt, dpkt.ethernet.ETH_TYPE_IP6)), {EXIT_A})), 1)

    def test_traffic_after_kill_is_a_finding(self):
        """After the tunnel is killed, a packet outside the allowlist (here DNS to the lab dns) is a
        leak, even though the torrents were held in time."""
        p = self.pcap(udp("10.88.0.10", 1, 51820), udp("10.88.0.10", 1, 51820), udp("10.88.0.23", 1, 53))  # ts 1000..1002
        f = check_kill_timing(p, 1000.5, {EXIT_A}, stale_after_s=20, held_at=1010.0)
        self.assertEqual(len(f), 1)
        self.assertEqual(f[0].check, "egress")
        self.assertIn("10.88.0.23:53", f[0].detail)

    def test_quiet_after_kill_is_clean(self):
        """Leaks before the kill belong to another window; quiet after it and held in time is clean."""
        p = self.pcap(udp("10.88.0.23", 1, 53), udp("10.88.0.10", 1, 51820))  # DNS at 1000, WG at 1001
        self.assertEqual(check_kill_timing(p, 1000.5, {EXIT_A}, stale_after_s=20, held_at=1025.0), [])

    def test_wireguard_keepalive_attempts_to_endpoint_are_clean(self):
        """While the tunnel is dead the client keeps retrying handshakes/keepalives to its endpoint:
        allowed traffic, not a leak."""
        p = self.pcap(*[udp("10.88.0.10", 45000, 51820, data=b"\x01" + b"\x00" * 147) for _ in range(6)])
        self.assertEqual(check_kill_timing(p, 999.0, {EXIT_A}, stale_after_s=20, held_at=1020.0), [])

    def test_held_too_late_or_never_is_a_finding(self):
        """Torrents must report held within stale_after + held_within_s of the kill."""
        p = self.pcap(udp("10.88.0.10", 1, 51820))
        late = check_kill_timing(p, 1000.0, {EXIT_A}, stale_after_s=20, held_at=1031.0, held_within_s=10)
        self.assertEqual([x.check for x in late], ["kill_timing"])
        self.assertIn("31", late[0].detail)
        never = check_kill_timing(p, 1000.0, {EXIT_A}, stale_after_s=20, held_at=None)
        self.assertEqual([x.check for x in never], ["kill_timing"])
        self.assertIn("never", never[0].detail)
        self.assertEqual(check_kill_timing(p, 1000.0, {EXIT_A}, stale_after_s=20, held_at=1030.0, held_within_s=10), [])

    def test_lock_released_while_still_cut_is_a_finding(self):
        """Once held, the torrents must stay held until the drop is lifted: a sample with the lock
        off (killed False) while the tunnel is still cut is a finding, reported once."""
        p = self.pcap(udp("10.88.0.10", 1, 51820))
        samples = [(1015.0, True, "stale", 21), (1016.0, False, "up", 0), (1017.0, False, "up", 1)]
        f = check_kill_timing(p, 1000.0, {EXIT_A}, stale_after_s=20, held_at=1015.0, samples=samples)
        self.assertEqual([x.check for x in f], ["kill_timing"])
        self.assertIn("released", f[0].detail)
        self.assertIn("2 of 3", f[0].detail)
        ok = [(1015.0, True, "stale", 21), (1016.0, True, "stale", 22)]
        self.assertEqual(check_kill_timing(p, 1000.0, {EXIT_A}, stale_after_s=20, held_at=1015.0, samples=ok), [])

    def test_recovered_needs_positive_evidence(self):
        """After the drop is lifted, "route up, not killed, not queued" alone proves nothing (the
        lock may never have held): recovery needs rx bytes or the torrent's progress to grow past
        their values at the restore."""
        at_restore = {"rx": 1000, "progress": 0.30}
        vacuous = {"route": "up", "killed": False, "status": "downloading", "rx": 1000, "progress": 0.30}
        self.assertFalse(recovered(at_restore, vacuous))
        self.assertTrue(recovered(at_restore, dict(vacuous, rx=5000)))
        self.assertTrue(recovered(at_restore, dict(vacuous, progress=0.31)))
        self.assertFalse(recovered(at_restore, dict(vacuous, rx=5000, route="stale")))
        self.assertFalse(recovered(at_restore, dict(vacuous, rx=5000, killed=True)))
        self.assertFalse(recovered(at_restore, dict(vacuous, rx=5000, status="queued")))

    def test_check_recovery(self):
        """After the drop is lifted the tunnel must be back (route up, torrents released) in time."""
        self.assertEqual(check_recovery(1000.0, 1020.0, 60, "s"), [])
        self.assertEqual([x.check for x in check_recovery(1000.0, None, 60, "s")], ["recovery"])
        self.assertEqual(len(check_recovery(1000.0, 1061.0, 60, "s")), 1)

    def test_non_ethernet_capture_is_refused(self):
        """The frame parser assumes Ethernet; any other link type must fail loudly, not read as clean."""
        p = self.pcap(b"\x45" + b"\x00" * 27, linktype=dpkt.pcap.DLT_RAW)
        with self.assertRaises(ValueError):
            check_egress(p, {EXIT_A})
        with self.assertRaises(ValueError):
            count_udp_to(p, EXIT_A)

    def test_identity_is_an_allowlist(self):
        """Only the exits and the lab servers may appear as sources. The client's raw LAN address
        or any unknown address is a finding, like the gateway."""
        logs = [{"server": "dns", "ts": 1.0, "src": "10.77.0.10", "name": "tracker.lab"},
                {"server": "canary", "ts": 2.0, "src": "172.17.0.1"},
                {"server": "tracker", "ts": 3.0, "src": "10.88.0.22"}]
        f = check_identity(logs, "s")
        self.assertEqual(len(f), 2)
        self.assertTrue(f[0].detail.startswith("dns saw 10.77.0.10"))
        self.assertTrue(f[1].detail.startswith("canary saw 172.17.0.1"))

    def test_unparseable_log_lines_are_findings(self):
        """A log line that is not a JSON object with a source cannot be checked: report it."""
        fd, path = tempfile.mkstemp(suffix=".log")
        with os.fdopen(fd, "w") as f:
            f.write('{"server": "dns", "ts": 1.0, "src": "10.88.0.10"}\n{"server": "dns", "ts"\n[1,2]\n\n')
        self.addCleanup(os.remove, path)
        entries, bad = load_logs([path])
        self.assertEqual(len(entries), 1)
        self.assertEqual(len(bad), 2)
        self.assertEqual({b.check for b in bad}, {"logs"})

    def test_split_control_pins_identity_signatures(self):
        """Gateway entries in the control window only count as the control's own if they are the
        control's request: the tracker path /positive-control, the dns name control.lab."""
        own_t = Finding("identity", "positive_control", "tracker saw 10.88.0.1 at 1.000: {'proto': 'http', 'path': '/positive-control'}")
        own_d = Finding("identity", "positive_control", "dns saw 10.88.0.1 at 1.000: {'name': 'control.lab', 'qtype': 1}")
        other_t = Finding("identity", "positive_control", "tracker saw 10.88.0.1 at 1.000: {'proto': 'http', 'path': '/announce'}")
        other_d = Finding("identity", "positive_control", "dns saw 10.88.0.1 at 1.000: {'name': 'tracker.lab', 'qtype': 1}")
        real, missing = split_control([own_t, own_d, other_t, other_d], "positive_control", CONTROL_SIGNATURES)
        self.assertEqual(real, [other_t, other_d])
        self.assertNotIn("tracker logs the real address", missing)
        self.assertNotIn("dns logs the real address", missing)

    def test_split_control_on_real_check_output(self):
        """End to end on real analyzer output: a capture and logs holding exactly the control's
        leaks give no real finding and nothing missing."""
        tcp = dpkt.tcp.TCP(sport=40000, dport=8000, flags=dpkt.tcp.TH_SYN)
        v6 = ip6("fd77::a", "fd77::1", dpkt.ip.IP_PROTO_UDP, dpkt.udp.UDP(sport=1, dport=9999, data=b"x"))
        p = self.pcap(eth(ip4(CLIENT_IP, "10.88.0.20", dpkt.ip.IP_PROTO_TCP, tcp)), udp("10.88.0.23", 1, 53),
                      udp("10.88.0.10", 1, 9999), eth(v6, dpkt.ethernet.ETH_TYPE_IP6))
        logs = [{"server": "tracker", "ts": 1.0, "src": "10.88.0.1", "proto": "http", "path": "/positive-control"},
                {"server": "dns", "ts": 1.0, "src": "10.88.0.1", "name": "control.lab", "qtype": 1}]
        fs = check_egress(p, {EXIT_A}, "positive_control") + check_identity(logs, "positive_control")
        self.assertEqual(split_control(fs, "positive_control", CONTROL_SIGNATURES), ([], []))

    # ---- Task 12b, F2: the kernel's port-unreachable answer to the WireGuard endpoint ----

    def unreach(self, dst="10.88.0.10", icmp_type=3, code=3, qsrc="10.88.0.10", qdst=CLIENT_IP, qsport=51820,
                qproto=dpkt.ip.IP_PROTO_UDP, cut=None):
        """A client ICMP packet to `dst` quoting a datagram (`qsrc`:`qsport` -> `qdst`:45000, proto
        `qproto`) the way Linux answers a datagram to a closed UDP port; `cut` truncates the quote."""
        l4 = (dpkt.udp.UDP(sport=qsport, dport=45000, data=b"\x04" + b"\x00" * 31) if qproto == dpkt.ip.IP_PROTO_UDP
              else dpkt.tcp.TCP(sport=qsport, dport=45000, flags=dpkt.tcp.TH_ACK))
        quoted = bytes(ip4(qsrc, qdst, qproto, l4))[:cut]
        icmp = dpkt.icmp.ICMP(type=icmp_type, code=code, data=b"\x00\x00\x00\x00" + quoted)
        return eth(ip4(CLIENT_IP, dst, dpkt.ip.IP_PROTO_ICMP, icmp))

    def test_port_unreachable_answer_to_endpoint_wireguard_is_clean(self):
        """F2: after the daemon closes its WireGuard socket, datagrams still in flight from the
        endpoint get the kernel's ICMP port unreachable. It goes only to the endpoint, which
        already knows the client's address, so this one narrow shape is allowed."""
        self.assertEqual(check_egress(self.pcap(self.unreach(), self.unreach()), {EXIT_A}), [])

    def test_other_icmp_shapes_stay_findings(self):
        """Everything outside the narrow F2 shape stays a finding: another unreachable code, an
        echo, a destination that is not the endpoint, a quote not from the WireGuard port, a TCP
        quote, a quote not addressed to the client, and a quote too short to check."""
        cases = {
            "type 3 code 1": self.unreach(code=1),
            "echo request": self.unreach(icmp_type=8, code=0),
            "destination not an endpoint": self.unreach(dst="10.88.0.20", qsrc="10.88.0.20"),
            "quoted source port not 51820": self.unreach(qsport=9999),
            "quoted proto TCP": self.unreach(qproto=dpkt.ip.IP_PROTO_TCP),
            "quoted destination not the client": self.unreach(qdst="10.77.0.99"),
            "quoted source not the endpoint": self.unreach(qsrc="10.88.0.11"),
            "truncated quote": self.unreach(cut=24),
            "to the gateway": self.unreach(dst="10.77.0.1"),
        }
        for name, frame in cases.items():
            with self.subTest(name):
                f = check_egress(self.pcap(frame), {EXIT_A})
                self.assertEqual(len(f), 1, f)
                self.assertIn("ICMP", f[0].detail)

    # ---- Task 14: the swap-gap check (rotation between exits A and B) ----

    def test_swap_packets_to_old_and_new_endpoint_are_clean(self):
        """During a swap WireGuard to the old (A) and the new (B) endpoint may both leave, and the
        F2 port unreachable to either endpoint about its own WireGuard datagram stays allowed."""
        p = self.pcap(udp("10.88.0.10", 45000, 51820), udp("10.88.0.11", 45001, 51820),
                      self.unreach(dst="10.88.0.11", qsrc="10.88.0.11"), self.unreach())
        self.assertEqual(check_swap_gap(p, (1000.0, 1003.0), [EXIT_A, EXIT_B]), [])

    def test_swap_packets_to_third_ip_is_a_finding(self):
        """Anything to a third address inside the swap window is a swap_gap finding."""
        p = self.pcap(udp("10.88.0.10", 45000, 51820), udp("10.88.0.12", 45000, 51820), udp("10.88.0.11", 45000, 51820))
        f = check_swap_gap(p, (1000.0, 1002.0), [EXIT_A, EXIT_B], "rotation_swap")
        self.assertEqual(len(f), 1, f)
        self.assertEqual((f[0].check, f[0].scenario), ("swap_gap", "rotation_swap"))
        self.assertIn("10.88.0.12:51820", f[0].detail)

    def test_swap_gap_only_looks_inside_the_window(self):
        """A packet outside the swap window is for the other checks, not this one."""
        p = self.pcap(udp("10.88.0.20", 45000, 6969), udp("10.88.0.10", 45000, 51820))
        self.assertEqual(check_swap_gap(p, (1001.0, 1001.0), [EXIT_A, EXIT_B]), [])

    def test_closed_endpoint_allows_only_its_port_unreachable(self):
        """After rotating away from exit B its tunnel is closed: B's datagrams still in flight get
        the F2 port unreachable (allowed via `closed`), but UDP to B stays a finding."""
        p = self.pcap(self.unreach(dst="10.88.0.11", qsrc="10.88.0.11"))
        self.assertEqual(check_egress(p, {EXIT_A}, closed={EXIT_B}), [])
        self.assertEqual(len(check_egress(p, {EXIT_A})), 1)
        f = check_egress(self.pcap(udp("10.88.0.11", 45000, 51820)), {EXIT_A}, closed={EXIT_B})
        self.assertEqual(len(f), 1, f)

    def test_swap_gap_port_unreachable_stays_pinned_to_one_endpoint(self):
        """With both endpoints allowed, an unreachable to A quoting B's datagram is still a finding:
        each endpoint may only hear about its own WireGuard flow."""
        f = check_swap_gap(self.pcap(self.unreach(qsrc="10.88.0.11")), (1000.0, 1000.0), [EXIT_A, EXIT_B])
        self.assertEqual(len(f), 1, f)
        self.assertIn("ICMP", f[0].detail)


def announce(ts, src="10.88.0.10", peer_id="AbCdEfGhIjKlMnOpQrSt", key="1a2b3c4d", port=1, ua=None, proto="udp"):
    """A synthetic tracker log entry (the shape tracker.py writes)."""
    e = {"server": "tracker", "ts": ts, "src": src, "proto": proto, "path": "/announce" if proto == "http" else None,
         "peer_id": peer_id, "key": key, "port": port}
    if ua is not None:
        e["user_agent"] = ua
    return e


def handshake(ts, src="10.88.0.10", peer_id="ZyXwVuTsRqPoNmLkJiHg", v=None, p=None, encrypted=False):
    """A synthetic observer log entry (the shape observer.py writes for a plaintext handshake)."""
    e = {"server": "observer", "ts": ts, "src": src, "encrypted": encrypted}
    if not encrypted:
        e.update(peer_id=peer_id, reserved="0000000000100005")
        if v is not None:
            e["v"] = v
        if p is not None:
            e["p"] = p
    return e


class Linkability(unittest.TestCase):
    """check_linkability (spec 7.3 check 3): identities must not survive a rotation and must not
    name the client."""
    MARKS = [2000.0]  # one rotation (rotate_now's reply) at t=2000; exit A before, exit B after

    def check(self, tracker, observer, marks=MARKS):
        """Runs the check and returns the findings' details (all must be check "linkability")."""
        found = check_linkability(tracker, observer, marks, scenario="identity_rotation")
        self.assertTrue(all(f.check == "linkability" and f.scenario == "identity_rotation" for f in found), found)
        return [f.detail for f in found]

    def test_same_peer_id_across_rotation_is_a_finding(self):
        """The tracker sees one peer_id before and after the swap: it can link both sessions."""
        d = self.check([announce(1000, key="11111111"), announce(3000, src="10.88.0.11", key="22222222")], [])
        self.assertEqual(len(d), 1, d)
        self.assertIn("peer_id", d[0])

    def test_same_tracker_key_across_rotation_is_a_finding(self):
        """A tracker key that survives the swap links the sessions even with a new peer_id."""
        d = self.check([announce(1000, peer_id="A" * 20), announce(3000, src="10.88.0.11", peer_id="B" * 20)], [])
        self.assertEqual(len(d), 1, d)
        self.assertIn("key", d[0])

    def test_different_identity_is_clean(self):
        """New peer_id and key after the swap, no version string, a generic User-Agent, the proxied
        placeholder port 1 and no listen port in the extension handshake: nothing links or names."""
        tracker = [announce(1000, peer_id="A" * 20, key="11111111"), announce(1060, peer_id="A" * 20, key="11111111"),
                   announce(3000, src="10.88.0.11", peer_id="B" * 20, key="22222222", proto="http", ua="curl/7.81.0")]
        observer = [handshake(1001, peer_id="C" * 20), handshake(3001, src="10.88.0.11", peer_id="D" * 20),
                    handshake(3002, src="10.88.0.11", encrypted=True)]
        self.assertEqual(self.check(tracker, observer), [])

    def test_same_identity_from_two_exits_is_a_finding_without_a_mark(self):
        """The old identity showing up on the new exit links the exits, even inside one segment
        (an announce of the old session that slipped out through the new tunnel)."""
        d = self.check([announce(1000), announce(1500, src="10.88.0.11")], [], marks=[])
        self.assertEqual(len(d), 2, d)  # peer_id and key
        self.assertTrue(any("10.88.0.11" in x for x in d), d)

    def test_new_identity_inside_the_swap_window_is_clean(self):
        """The new session announces before rotate_now replies: inside the swap window (call to
        reply) an entry belongs to no session, so the new identity there is not "two sessions"."""
        tracker = [announce(1000, peer_id="A" * 20, key="11111111"),
                   announce(1999, src="10.88.0.11", peer_id="B" * 20, key="22222222"),
                   announce(3000, src="10.88.0.11", peer_id="B" * 20, key="22222222")]
        self.assertEqual(self.check(tracker, [], marks=[(1990.0, 2000.0)]), [])
        # The old identity inside the window but from the new exit is still caught (exit rule).
        leak = [announce(1000), announce(1995, src="10.88.0.11")]
        self.assertEqual(len(self.check(leak, [], marks=[(1990.0, 2000.0)])), 2)

    def test_same_observer_peer_id_across_rotation_is_a_finding(self):
        """The peer_id a peer sees in the handshake counts too."""
        d = self.check([], [handshake(1000), handshake(3000, src="10.88.0.11")])
        self.assertEqual(len(d), 1, d)
        self.assertIn("peer_id", d[0])

    def test_client_fingerprint_is_a_finding(self):
        """A libtorrent peer_id prefix, a version string or a User-Agent naming Nyx or libtorrent
        identifies the client, rotation or not."""
        cases = [([announce(1000, peer_id="-LT20B0-abcdefghijkl")], []),
                 ([], [handshake(1000, peer_id="-LT20B0-abcdefghijkl")]),
                 ([], [handshake(1000, v="libtorrent/2.0.11.0")]),
                 ([], [handshake(1000, v="Nyx/0.1")]),
                 ([announce(1000, proto="http", ua="Nyx/0.1")], [])]
        for tracker, observer in cases:
            d = self.check(tracker, observer)
            self.assertEqual(len(d), 1, (tracker, observer, d))
            self.assertIn("names the client", d[0])

    def test_same_version_string_across_rotation_is_a_finding(self):
        """Even a neutral `v` links the sessions when it is the same unusual string on both sides."""
        d = self.check([], [handshake(1000, peer_id="C" * 20, v="Foo 1.2"), handshake(3000, src="10.88.0.11", peer_id="D" * 20, v="Foo 1.2")])
        self.assertEqual(len(d), 1, d)
        self.assertIn("v", d[0])

    def test_announced_listen_port_is_a_finding(self):
        """A proxied client cannot take inbound connections, so any concrete port it announces is
        its real listen port (or a stable token): a finding in the handshake and at the tracker.
        Port 1 (libtorrent's placeholder when it cannot accept) and no port are clean."""
        d = self.check([announce(1000, port=51413)], [handshake(1000, p=6881)])
        self.assertEqual(len(d), 2, d)
        self.assertTrue(all("listen port" in x for x in d), d)
        self.assertEqual(self.check([announce(1000, port=1), announce(1001, port="1", proto="http")], [handshake(1000)]), [])


class Report(unittest.TestCase):
    """report.write: sections and exit code."""

    def write(self, scenarios, findings, missing, partial=None):
        """Writes a report to a temp file; returns (exit code, text)."""
        fd, path = tempfile.mkstemp(suffix=".md")
        os.close(fd)
        self.addCleanup(os.remove, path)
        code = report.write(path, scenarios, findings, [], missing, partial=partial)
        with open(path, encoding="utf-8") as f:
            return code, f.read()

    def test_subset_run_verdict_says_partial(self):
        """A LAB_ONLY subset run must not read as a clean lab: its verdict line names it a partial
        run and which scenarios it was limited to."""
        _, text = self.write([("s", True, "ok")], [], [], partial="steady_download,bait")
        result = next(l for l in text.splitlines() if l.startswith("Result:"))
        self.assertIn("PARTIAL RUN (LAB_ONLY=steady_download,bait)", result)
        _, full = self.write([("s", True, "ok")], [], [])
        self.assertNotIn("PARTIAL", full)

    def test_lab_only_names_are_validated(self):
        """LAB_ONLY: empty means every scenario; known names select a subset; any unknown name is
        refused up front with the list of valid names (a typo must not silently skip everything)."""
        valid = ["steady_download", "bait"]
        self.assertEqual(report.lab_only("", valid), set())
        self.assertEqual(report.lab_only("bait, steady_download", valid), {"bait", "steady_download"})
        with self.assertRaises(ValueError) as e:
            report.lab_only("bait,steady_downlaod", valid)
        self.assertIn("steady_downlaod", str(e.exception))
        self.assertIn("steady_download, bait", str(e.exception))

    def test_control_missing_fails_and_says_blind(self):
        """A blind lab fails the run even with every scenario run and no finding."""
        code, text = self.write([("s", True, "ok")], [], ["DNS query to the lab dns"])
        self.assertEqual(code, 1)
        self.assertIn("LAB BLIND", text)
        self.assertIn("DNS query to the lab dns: NOT DETECTED", text)

    def test_clean_run_passes(self):
        """Everything ran, nothing found, control complete: exit 0."""
        self.assertEqual(self.write([("s", True, "ok")], [], [])[0], 0)

    def test_infrastructure_failures_are_separate_from_leaks(self):
        """The verdict counts scenarios that could not run apart from findings (leaks)."""
        code, text = self.write([("a", False, "boom"), ("b", True, "ok")], [Finding("egress", "b", "UDP -> x")], [])
        self.assertEqual(code, 1)
        self.assertIn("1 scenario failed (infrastructure)", text)
        self.assertIn("1 finding (leaks)", text)


if __name__ == "__main__":
    unittest.main()
