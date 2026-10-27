"""analyzer.py - the leak lab's pass/fail checks, as pure functions over a pcap file or parsed
server logs (no Docker here, so test_analyzer.py runs them on synthetic input).

check_egress: the client's sidecar captures only packets LEAVING the client (tcpdump -Q out). The
only allowed traffic is UDP to an allowed WireGuard endpoint, plus link housekeeping: ARP, and
ICMPv6 neighbour discovery from a link-local address to a link-local or multicast address.
Everything else is a finding, one per flow.

check_identity: 10.88.0.1 is the NAT gateway's internet-side address, i.e. the client's real ISP
identity. Any server log entry with that source caught the client outside the tunnel."""
import ipaddress
import socket
from typing import NamedTuple

import dpkt

GATEWAY_IDENTITY = "10.88.0.1"
# Well-known ports turned into a label in findings, so the report says what leaked.
PORT_LABELS = {53: "DNS", 67: "DHCP", 68: "DHCP", 123: "NTP", 1900: "SSDP/UPnP", 5351: "NAT-PMP",
               5353: "mDNS", 5355: "LLMNR", 6771: "LSD", 6881: "BitTorrent", 6969: "UDP tracker",
               8000: "HTTP tracker", 80: "HTTP", 443: "HTTPS", 51820: "WireGuard"}
# ICMPv6 router solicitation, neighbour solicitation, neighbour advertisement.
ND_TYPES = {133, 135, 136}


class Finding(NamedTuple):
    """One check failure: which check, in which scenario, and a human-readable detail line."""
    check: str
    scenario: str
    detail: str


def _label(port):
    """' (mDNS)'-style suffix for a well-known port, else ''."""
    return f" ({PORT_LABELS[port]})" if port in PORT_LABELS else ""


def _housekeeping_v6(ip6):
    """True for ICMPv6 neighbour discovery from link-local/unspecified to link-local/multicast."""
    if ip6.nxt != dpkt.ip.IP_PROTO_ICMP6 or not isinstance(ip6.data, dpkt.icmp6.ICMP6) or ip6.data.type not in ND_TYPES:
        return False
    src = ipaddress.IPv6Address(ip6.src)
    dst = ipaddress.IPv6Address(ip6.dst)
    return (src.is_link_local or src.is_unspecified) and (dst.is_link_local or dst.is_multicast)


def _flow(frame, allowed):
    """Classifies one Ethernet frame: None if allowed, else a flow key string for the finding."""
    eth = dpkt.ethernet.Ethernet(frame)
    pkt = eth.data
    if eth.type == dpkt.ethernet.ETH_TYPE_ARP:
        return None
    if isinstance(pkt, dpkt.ip.IP):
        dst = socket.inet_ntoa(pkt.dst)
        l4 = pkt.data
        if isinstance(l4, dpkt.udp.UDP):
            if (dst, l4.dport) in allowed:
                return None
            return f"UDP -> {dst}:{l4.dport}{_label(l4.dport)}"
        if isinstance(l4, dpkt.tcp.TCP):
            return f"TCP -> {dst}:{l4.dport}{_label(l4.dport)}"
        if isinstance(l4, dpkt.icmp.ICMP):
            return f"ICMP type {l4.type} -> {dst}"
        return f"IPv4 proto {pkt.p} -> {dst}"
    if isinstance(pkt, dpkt.ip6.IP6):
        if _housekeeping_v6(pkt):
            return None
        dst = socket.inet_ntop(socket.AF_INET6, pkt.dst)
        l4 = pkt.data
        port = getattr(l4, "dport", None)
        what = f"next header {pkt.nxt}" if port is None else f"port {port}{_label(port)}"
        return f"IPv6 -> [{dst}] {what}"
    return f"ethertype 0x{eth.type:04x}"


def check_egress(pcap, allowed_endpoints, scenario="", since=0.0, until=float("inf")):
    """Findings for every packet in `pcap` (path) whose time is in [since, until] and that is not
    UDP to one of `allowed_endpoints` ({(ip, port)}) or housekeeping. One finding per flow, with
    the packet count and the first time seen."""
    flows = {}
    with open(pcap, "rb") as f:
        for ts, frame in dpkt.pcap.Reader(f):
            if not since <= ts <= until:
                continue
            try:
                key = _flow(frame, allowed_endpoints)
            except (dpkt.UnpackError, ValueError) as e:
                key = f"unparseable frame ({e})"
            if key is not None:
                first, n = flows.get(key, (ts, 0))
                flows[key] = (first, n + 1)
    return [Finding("egress", scenario, f"{key}: {n} packets, first at {first:.3f}") for key, (first, n) in flows.items()]


def count_udp_to(pcap, endpoint, since=0.0, until=float("inf")):
    """Number of captured UDP packets to `endpoint` (ip, port) in [since, until]. The runner uses it
    to prove the capture saw the tunnel at all (an empty capture would pass check_egress)."""
    n = 0
    with open(pcap, "rb") as f:
        for ts, frame in dpkt.pcap.Reader(f):
            pkt = dpkt.ethernet.Ethernet(frame).data
            if since <= ts <= until and isinstance(pkt, dpkt.ip.IP) and isinstance(pkt.data, dpkt.udp.UDP):
                n += (socket.inet_ntoa(pkt.dst), pkt.data.dport) == endpoint
    return n


def capture_live(pcap, endpoint, since, until, tail=30.0):
    """Proves the capture was recording through [since, until], so a clean check_egress over that
    window means something. Needs a WireGuard packet to `endpoint` in the window and an outbound
    packet in its last `tail` seconds (PersistentKeepalive is 25 s, so a live tunnel always has
    one). Returns (True, "") or (False, reason)."""
    if count_udp_to(pcap, endpoint, since, until) == 0:
        return False, f"no WireGuard packet to {endpoint[0]}:{endpoint[1]} in the capture window: capture dead or empty"
    with open(pcap, "rb") as f:
        last = max((ts for ts, _ in dpkt.pcap.Reader(f) if ts <= until), default=0.0)
    if last < until - tail:
        return False, f"no packet captured in the last {tail:.0f} s of the window (last at {last:.3f}): capture stalled"
    return True, ""


# The positive control's deliberate leaks: (name, check, start of the finding's detail). Only
# findings in the control's own window that start with one of these are set aside.
CONTROL_SIGNATURES = [
    ("TCP to the tracker", "egress", "TCP -> 10.88.0.20:8000"),
    ("DNS query to the lab dns", "egress", "UDP -> 10.88.0.23:53"),
    ("UDP to exit A off the WireGuard port", "egress", "UDP -> 10.88.0.10:9999"),
    ("tracker logs the real address", "identity", f"tracker saw {GATEWAY_IDENTITY}"),
    ("dns logs the real address", "identity", f"dns saw {GATEWAY_IDENTITY}"),
]


def split_control(findings, control_scenario, signatures):
    """Separates the positive control's expected findings from real ones. A finding is set aside
    only if it belongs to `control_scenario` AND matches a signature; everything else stays real.
    Returns (real findings, names of expected signatures that were NOT detected)."""
    def match(f, c, p):
        """`f` is in the control window, of check `c`, and its detail is `p` followed by a word end
        (so "...:53" does not match "...:5353")."""
        return (f.scenario == control_scenario and f.check == c and f.detail.startswith(p)
                and f.detail[len(p):len(p) + 1] in (" ", ":", ""))

    real = [f for f in findings if not any(match(f, c, p) for _, c, p in signatures)]
    missing = [name for name, c, p in signatures if not any(match(f, c, p) for f in findings)]
    return real, missing


def check_identity(server_logs, scenario=""):
    """Findings for every server log entry (dicts with server/ts/src) whose source is the client's
    real address, the gateway 10.88.0.1."""
    out = []
    for e in server_logs:
        if e.get("src") == GATEWAY_IDENTITY:
            extra = {k: v for k, v in e.items() if k not in ("server", "ts", "src", "headers")}
            out.append(Finding("identity", scenario, f"{e.get('server')} saw {GATEWAY_IDENTITY} at {e.get('ts', 0):.3f}: {extra}"))
    return out
