"""analyzer.py - the leak lab's pass/fail checks, as pure functions over a pcap file or parsed
server logs (no Docker here, so test_analyzer.py runs them on synthetic input).

check_egress: the client's sidecar captures only packets LEAVING the client (tcpdump -Q out). The
only allowed traffic is UDP to an allowed WireGuard endpoint, plus link housekeeping: ARP, and
ICMPv6 neighbour discovery / MLD reports to a link-local or link-scope multicast (ff02::) address.
Everything else is a finding, one per flow.

check_identity: an allowlist of sources the lab servers may see: the two WireGuard exits and the lab
servers themselves. 10.88.0.1 is the NAT gateway's internet-side address, i.e. the client's real
ISP identity; it, the raw client address or anything else is a finding.

check_kill_timing / check_recovery: the tunnel_killed_midtransfer scenario's checks (nothing but
WireGuard to the endpoint after the kill, torrents held in time, tunnel back after the drop)."""
import ipaddress
import json
import os
import socket
from typing import NamedTuple

import dpkt

GATEWAY_IDENTITY = "10.88.0.1"
# Sources a lab server may log: the exits (tunnelled client traffic) and the lab servers'
# own addresses (tracker, seeder, observer, dns, canary talking to each other).
ALLOWED_SOURCES = {"10.88.0.10", "10.88.0.11", "10.88.0.20", "10.88.0.21", "10.88.0.22", "10.88.0.23", "10.88.0.24"}
# Well-known ports turned into a label in findings, so the report says what leaked.
PORT_LABELS = {53: "DNS", 67: "DHCP", 68: "DHCP", 123: "NTP", 1900: "SSDP/UPnP", 5351: "NAT-PMP",
               5353: "mDNS", 5355: "LLMNR", 6771: "LSD", 6881: "BitTorrent", 6969: "UDP tracker",
               8000: "HTTP tracker", 80: "HTTP", 443: "HTTPS", 51820: "WireGuard"}
# ICMPv6 link housekeeping: router solicitation, neighbour solicitation and advertisement, and
# MLDv2 listener reports (sent behind a hop-by-hop header). The kernel's IPv6 "ARP".
ND_TYPES = {133, 135, 136, 143}


class Finding(NamedTuple):
    """One check failure: which check, in which scenario, and a human-readable detail line."""
    check: str
    scenario: str
    detail: str


def _label(port):
    """' (mDNS)'-style suffix for a well-known port, else ''."""
    return f" ({PORT_LABELS[port]})" if port in PORT_LABELS else ""


def _housekeeping_v6(ip6):
    """True for ICMPv6 neighbour discovery or MLD to a link-local or link-scope multicast address
    (ff02::/16), from any source: such a packet cannot leave the LAN, like ARP. The final next
    header (`p`, past a hop-by-hop header) must be ICMPv6."""
    if getattr(ip6, "p", ip6.nxt) != dpkt.ip.IP_PROTO_ICMP6 or not isinstance(ip6.data, dpkt.icmp6.ICMP6) or ip6.data.type not in ND_TYPES:
        return False
    dst = ipaddress.IPv6Address(ip6.dst)
    return dst.is_link_local or dst in ipaddress.IPv6Network("ff02::/16")


def _reader(f):
    """A pcap reader over open file `f` that refuses anything but Ethernet captures: _flow parses
    Ethernet frames, so another link type would mis-parse every packet."""
    r = dpkt.pcap.Reader(f)
    if r.datalink() != dpkt.pcap.DLT_EN10MB:
        raise ValueError(f"capture link type {r.datalink()} is not Ethernet (DLT_EN10MB)")
    return r


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
        for ts, frame in _reader(f):
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
        for ts, frame in _reader(f):
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
        last = max((ts for ts, _ in _reader(f) if ts <= until), default=0.0)
    if last < until - tail:
        return False, f"no packet captured in the last {tail:.0f} s of the window (last at {last:.3f}): capture stalled"
    return True, ""


# The positive control's deliberate leaks: (name, check, start of the finding's detail, text the
# detail must also contain). Only findings in the control's own window that match are set aside;
# the identity ones are pinned to the control's own request (path / query name).
CONTROL_SIGNATURES = [
    ("TCP to the tracker", "egress", "TCP -> 10.88.0.20:8000", ""),
    ("DNS query to the lab dns", "egress", "UDP -> 10.88.0.23:53", ""),
    ("UDP to exit A off the WireGuard port", "egress", "UDP -> 10.88.0.10:9999", ""),
    ("UDP to the lab gateway over IPv6", "egress", "IPv6 -> [fd77::1] port 9999", ""),
    ("tracker logs the real address", "identity", f"tracker saw {GATEWAY_IDENTITY}", "'path': '/positive-control'"),
    ("dns logs the real address", "identity", f"dns saw {GATEWAY_IDENTITY}", "'name': 'control.lab'"),
]


def split_control(findings, control_scenario, signatures):
    """Separates the positive control's expected findings from real ones. A finding is set aside
    only if it belongs to `control_scenario` AND matches a signature; everything else stays real.
    Returns (real findings, names of expected signatures that were NOT detected)."""
    def match(f, c, p, needle):
        """`f` is in the control window, of check `c`, its detail is `p` followed by a word end
        (so "...:53" does not match "...:5353") and contains `needle`."""
        return (f.scenario == control_scenario and f.check == c and f.detail.startswith(p)
                and f.detail[len(p):len(p) + 1] in (" ", ":", "") and needle in f.detail)

    real = [f for f in findings if not any(match(f, *sig[1:]) for sig in signatures)]
    missing = [sig[0] for sig in signatures if not any(match(f, *sig[1:]) for f in findings)]
    return real, missing


def check_identity(server_logs, scenario="", allowed_sources=ALLOWED_SOURCES):
    """Findings for every server log entry (dicts with server/ts/src) whose source is not in
    `allowed_sources`: the gateway 10.88.0.1 (the client's real address), the raw client address
    or anything unknown."""
    out = []
    for e in server_logs:
        if e.get("src") not in allowed_sources:
            extra = {k: v for k, v in e.items() if k not in ("server", "ts", "src", "headers")}
            out.append(Finding("identity", scenario, f"{e.get('server')} saw {e.get('src')} at {e.get('ts', 0):.3f}: {extra}"))
    return out


def load_logs(paths):
    """Reads the servers' JSON-lines logs. Returns (entries, findings): every line that is not a
    JSON object with a `src` cannot be checked by check_identity, so it is a "logs" finding
    instead of being skipped. Blank lines are ignored."""
    entries, bad = [], []
    for path in paths:
        with open(path, encoding="utf-8", errors="replace") as f:
            for n, line in enumerate(f, 1):
                if not line.strip():
                    continue
                try:
                    e = json.loads(line)
                except ValueError:
                    e = None
                if isinstance(e, dict) and "src" in e:
                    entries.append(e)
                else:
                    bad.append(Finding("logs", "", f"{os.path.basename(path)} line {n} unparseable: {line.strip()[:120]}"))
    return entries, bad


def check_kill_timing(pcap, kill_time, allowed_endpoints, stale_after_s, held_at, held_within_s=10,
                      until=float("inf"), scenario="tunnel_killed_midtransfer", samples=()):
    """The kill-switch check. After `kill_time` (the WireGuard path was cut) no packet outside
    `allowed_endpoints` may leave (retries to the endpoint are fine), and the torrents must have
    reported held (`held_at`, None = never) within `stale_after_s` (the lock's stale threshold)
    plus `held_within_s`. `samples` are (time, killed, route, handshake age) readings taken from
    `held_at` until the drop was lifted: the lock must stay on all through (one finding if not)."""
    out = check_egress(pcap, allowed_endpoints, scenario, kill_time, until)
    off = [s for s in samples if not s[1]]
    if off:
        t, _, route, age = off[0]
        out.append(Finding("kill_timing", scenario, f"lock released while the tunnel was still cut: {len(off)} of {len(samples)} "
                                                    f"samples, first {t - kill_time:.1f} s after the kill (route {route}, handshake age {age} s)"))
    deadline = stale_after_s + held_within_s
    if held_at is None:
        out.append(Finding("kill_timing", scenario, f"torrents never reported held (deadline {deadline:.0f} s after the kill)"))
    elif held_at - kill_time > deadline:
        out.append(Finding("kill_timing", scenario, f"torrents held {held_at - kill_time:.1f} s after the kill (deadline {deadline:.0f} s)"))
    return out


def check_recovery(restore_time, recovered_at, within_s, scenario):
    """After the drop is lifted (`restore_time`) the tunnel must be back, route up and torrents
    released (`recovered_at`, None = never), within `within_s`."""
    if recovered_at is None:
        return [Finding("recovery", scenario, f"not recovered within {within_s:.0f} s of lifting the drop")]
    if recovered_at - restore_time > within_s:
        return [Finding("recovery", scenario, f"recovered {recovered_at - restore_time:.1f} s after lifting the drop (bound {within_s:.0f} s)")]
    return []
