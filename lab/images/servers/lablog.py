"""lablog.py - shared JSON-lines logger for the leak lab servers. Every entry has the server name,
a wall-clock timestamp and the source address, so the runner's identity check can read all logs
the same way. Writes /out/<name>.log, flushed per line so a crash loses nothing."""
import json
import os
import threading
import time

_lock = threading.Lock()


class LabLog:
    """Appends one JSON object per line to /out/<server>.log (OUT_DIR overrides /out)."""

    def __init__(self, server):
        """Opens (truncates) the log file for `server`: each lab run starts with an empty log."""
        self.server = server
        out = os.environ.get("OUT_DIR", "/out")
        os.makedirs(out, exist_ok=True)
        self.f = open(os.path.join(out, f"{server}.log"), "w", encoding="utf-8")

    def write(self, src, **fields):
        """Logs one event seen from IP `src` with any extra JSON-safe fields."""
        entry = {"server": self.server, "ts": time.time(), "src": src, **fields}
        with _lock:
            self.f.write(json.dumps(entry) + "\n")
            self.f.flush()


def text(b):
    """Bytes to a printable string for logs (latin-1 keeps every byte, escapes the rest)."""
    return b.decode("latin-1").encode("unicode_escape").decode("ascii") if isinstance(b, bytes) else b
