"""dns.py - leak lab DNS server on UDP :53. Logs every query (name, type, source IP) and answers A
records for the lab names; anything else gets NXDOMAIN. It is both the "ISP resolver" the client's
system would use and the resolver named in the VPN profiles (reached through the exits), so the
source IP tells which path a lookup took."""
import socket
import struct

from lablog import LabLog

LOG = LabLog("dns")
NAMES = {
    "tracker.lab": "10.88.0.20", "seeder.lab": "10.88.0.21", "observer.lab": "10.88.0.22",
    "dns.lab": "10.88.0.23", "canary.lab": "10.88.0.24",
}


def parse_question(msg):
    """Returns (name, qtype, end offset of the question) of the first question in a query."""
    i, labels = 12, []
    while msg[i]:
        n = msg[i]
        labels.append(msg[i + 1:i + 1 + n].decode("ascii", "replace"))
        i += n + 1
    qtype, _ = struct.unpack(">HH", msg[i + 1:i + 5])
    return ".".join(labels), qtype, i + 5


def answer(msg, name, qtype, qend):
    """Builds the reply: one A record (TTL 60) for a known lab name asked as A, else NXDOMAIN
    (or an empty NOERROR answer for other types of a known name)."""
    tid = msg[:2]
    ip = NAMES.get(name.lower().rstrip("."))
    question = msg[12:qend]
    if ip is None:
        return tid + struct.pack(">HHHHH", 0x8183, 1, 0, 0, 0) + question
    if qtype != 1:
        return tid + struct.pack(">HHHHH", 0x8180, 1, 0, 0, 0) + question
    rr = struct.pack(">HHHIH", 0xC00C, 1, 1, 60, 4) + socket.inet_aton(ip)
    return tid + struct.pack(">HHHHH", 0x8180, 1, 1, 0, 0) + question + rr


if __name__ == "__main__":
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("0.0.0.0", 53))
    while True:
        data, (ip, port) = s.recvfrom(4096)
        try:
            name, qtype, qend = parse_question(data)
        except (IndexError, struct.error):
            LOG.write(ip, sport=port, note="unparseable", raw=data[:64].hex())
            continue
        LOG.write(ip, sport=port, name=name, qtype=qtype)
        s.sendto(answer(data, name, qtype, qend), (ip, port))
