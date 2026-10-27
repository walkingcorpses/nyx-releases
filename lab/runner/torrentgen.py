"""torrentgen.py - makes the leak lab's test payload: a deterministic pseudo-random file and a v1
.torrent for it (single file, 256 KiB pieces) announcing to the lab tracker."""
import hashlib
import os
import random

from bencode import encode

PIECE = 256 * 1024


def make_payload(path, size, seed=7):
    """Writes `size` bytes of seeded pseudo-random data to `path` (same bytes on every run, so the
    info-hash is stable) and returns its SHA-256 hex digest."""
    rng = random.Random(seed)
    h = hashlib.sha256()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        left = size
        while left:
            chunk = rng.randbytes(min(left, 1 << 20))
            f.write(chunk)
            h.update(chunk)
            left -= len(chunk)
    return h.hexdigest()


def make_torrent(data_path, torrent_path, trackers):
    """Writes a single-file v1 torrent for `data_path` to `torrent_path`. `trackers` is a list of
    tiers (lists of URLs); the first URL is also `announce`. Returns the info-hash hex."""
    pieces = []
    with open(data_path, "rb") as f:
        while chunk := f.read(PIECE):
            pieces.append(hashlib.sha1(chunk).digest())
    info = {"name": os.path.basename(data_path), "length": os.path.getsize(data_path),
            "piece length": PIECE, "pieces": b"".join(pieces)}
    meta = {"announce": trackers[0][0], "announce-list": trackers, "info": info}
    with open(torrent_path, "wb") as f:
        f.write(encode(meta))
    return hashlib.sha1(encode(info)).hexdigest()
