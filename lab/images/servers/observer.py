"""observer.py - leak lab passive BitTorrent peer on :6881. Accepts a connection, records what any
peer learns about the client (source IP, peer_id as printable text and hex, reserved bits, and the
whole BEP 10 extension handshake decoded: `v` version string, `p` listen port, `yourip`, `reqq`,
`m`...), then closes. Never sends data pieces.
An MSE/PE-encrypted connection cannot be read; it is logged as such. MSE only wraps the same
handshake, so the identity scenario lets the client fall back to plaintext (encryption "Prefer")."""
import socket
import struct
import threading

from bencode import decode, encode
from lablog import LabLog, text

LOG = LabLog("observer")
PSTR = b"\x13BitTorrent protocol"
OUR_ID = b"-OB0001-labobserver1"


def recv_exact(c, n):
    """Reads exactly n bytes or raises ConnectionError."""
    buf = b""
    while len(buf) < n:
        chunk = c.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("closed")
        buf += chunk
    return buf


def plain(v):
    """A decoded bencode value made JSON-safe for the log: bytes become printable text (lablog.text),
    dict keys too, lists element by element."""
    if isinstance(v, dict):
        return {text(k): plain(x) for k, x in v.items()}
    if isinstance(v, list):
        return [plain(x) for x in v]
    return text(v)


def ext_handshake(c):
    """Reads peer messages until the extension handshake (id 20, ext 0); returns its dict or None."""
    for _ in range(16):
        (n,) = struct.unpack(">I", recv_exact(c, 4))
        if n == 0:
            continue
        msg = recv_exact(c, n)
        if msg[0] == 20 and msg[1] == 0:
            return decode(msg[2:])[0]
    return None


def session(c, addr):
    """One inbound peer: log the handshake, answer it so the peer sends its extension handshake,
    log that too, close."""
    entry = {"sport": addr[1]}
    try:
        c.settimeout(10)
        first = recv_exact(c, 20)
        if first != PSTR:
            entry.update(encrypted=True, first_bytes=first.hex())
            return
        rest = recv_exact(c, 48)
        reserved, info_hash, peer_id = rest[:8], rest[8:28], rest[28:48]
        entry.update(encrypted=False, reserved=reserved.hex(), info_hash=info_hash.hex(),
                     peer_id=text(peer_id), peer_id_hex=peer_id.hex(), extension_protocol=bool(reserved[5] & 0x10))
        c.sendall(PSTR + bytes([0, 0, 0, 0, 0, 0x10, 0, 0]) + info_hash + OUR_ID)
        hs = encode({"m": {}, "v": "lab-observer"})
        c.sendall(struct.pack(">IBB", len(hs) + 2, 20, 0) + hs)
        ext = ext_handshake(c)
        if ext is not None:
            yourip = ext.get(b"yourip", b"")
            entry.update(v=text(ext.get(b"v", b"")), p=ext.get(b"p"), reqq=ext.get(b"reqq"),
                         yourip=socket.inet_ntoa(yourip) if len(yourip) == 4 else text(yourip),
                         m=sorted(text(k) for k in ext.get(b"m", {})),
                         ext_keys=sorted(text(k) for k in ext), ext=plain(ext))
    except (OSError, ValueError, ConnectionError, IndexError) as e:
        entry["error"] = str(e)
    finally:
        LOG.write(addr[0], **entry)
        c.close()


if __name__ == "__main__":
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("0.0.0.0", 6881))
    srv.listen(16)
    while True:
        conn, peer = srv.accept()
        threading.Thread(target=session, args=(conn, peer), daemon=True).start()
