"""test_analyzer.py - unit tests for the leak lab analyzer, on tiny synthetic pcaps built with dpkt.
Run in the runner image: `python -m unittest test_analyzer`."""
import os
import socket
import tempfile
import unittest

import dpkt

from analyzer import Finding, check_egress, check_identity, count_udp_to

CLIENT_MAC = b"\x02\x42\x0a\x4d\x00\x0a"
GW_MAC = b"\x02\x42\x0a\x4d\x00\x01"
CLIENT_IP = "10.77.0.10"
EXIT_A = ("10.88.0.10", 51820)


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

    def pcap(self, *frames):
        """Writes `frames` to a temp pcap (one second apart) and returns its path."""
        fd, path = tempfile.mkstemp(suffix=".pcap")
        with os.fdopen(fd, "wb") as f:
            w = dpkt.pcap.Writer(f)
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


if __name__ == "__main__":
    unittest.main()
