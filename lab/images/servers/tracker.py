"""tracker.py - leak lab BitTorrent tracker: HTTP on :8000 (/announce) and UDP (BEP 15) on :6969.
Logs every announce with the source IP and everything a tracker could use to identify or link a
client: peer_id, port, ip=, key, event, User-Agent. Always answers with the two lab peers (the
seeder and the passive observer), so a client that announces gets someone to talk to."""
import socket
import struct
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from bencode import encode
from lablog import LabLog, text

LOG = LabLog("tracker")
# seeder 10.88.0.21 and observer 10.88.0.22, both on the BitTorrent port 6881.
PEERS = [("10.88.0.21", 6881), ("10.88.0.22", 6881)]
COMPACT = b"".join(socket.inet_aton(ip) + struct.pack(">H", p) for ip, p in PEERS)
INTERVAL = 60


def parse_query(raw):
    """Splits a raw query string into {name: bytes}; info_hash and peer_id are binary, so the
    values are unquoted to bytes rather than text."""
    out = {}
    for part in raw.split("&"):
        if part:
            k, _, v = part.partition("=")
            out[urllib.parse.unquote(k)] = urllib.parse.unquote_to_bytes(v)
    return out


class Announce(BaseHTTPRequestHandler):
    """HTTP tracker: logs the request and returns the lab peers in compact form."""

    def do_GET(self):
        """Handles /announce (and logs any other path too, as a probe)."""
        path, _, raw = self.path.partition("?")
        q = parse_query(raw)
        LOG.write(
            self.client_address[0], sport=self.client_address[1], proto="http", path=path,
            info_hash=q.get("info_hash", b"").hex(), peer_id=text(q.get("peer_id", b"")),
            port=text(q.get("port", b"")), ip=text(q.get("ip", b"")), key=text(q.get("key", b"")),
            event=text(q.get("event", b"")), user_agent=self.headers.get("User-Agent", ""),
            headers=dict(self.headers.items()), query=raw)
        body = encode({"interval": INTERVAL, "peers": COMPACT})
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        """Silences the default stderr access log (the JSON log has everything)."""


def udp_tracker():
    """BEP 15 UDP tracker loop: connect and announce actions; logs each announce."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("0.0.0.0", 6969))
    while True:
        data, (ip, port) = s.recvfrom(2048)
        if len(data) >= 16 and struct.unpack(">QII", data[:16])[1] == 0:  # connect
            _, _, tid = struct.unpack(">QII", data[:16])
            s.sendto(struct.pack(">IIQ", 0, tid, 0x1234ABCD5678), (ip, port))
        elif len(data) >= 98 and struct.unpack(">I", data[8:12])[0] == 1:  # announce
            tid = struct.unpack(">I", data[12:16])[0]
            info_hash, peer_id = data[16:36], data[36:56]
            event, ann_ip, key, num_want, aport = struct.unpack(">IIIiH", data[80:98])
            LOG.write(ip, sport=port, proto="udp", info_hash=info_hash.hex(), peer_id=text(peer_id),
                      port=aport, ip=socket.inet_ntoa(struct.pack(">I", ann_ip)), key=f"{key:08x}",
                      event=event, num_want=num_want, extensions=data[98:].hex())
            s.sendto(struct.pack(">IIIII", 1, tid, INTERVAL, 1, 1) + COMPACT, (ip, port))
        else:
            LOG.write(ip, sport=port, proto="udp", note="unknown packet", raw=data[:64].hex())


if __name__ == "__main__":
    threading.Thread(target=udp_tracker, daemon=True).start()
    ThreadingHTTPServer(("0.0.0.0", 8000), Announce).serve_forever()
