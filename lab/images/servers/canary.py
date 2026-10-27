"""canary.py - leak lab canary web server on :80. Stands in for RSS, search and update endpoints in
later scenarios: logs the source IP, method, path and every request header, then answers 200
with a tiny body."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from lablog import LabLog

LOG = LabLog("canary")


class Canary(BaseHTTPRequestHandler):
    """Logs any request and answers "ok"."""

    def handle_any(self):
        """Shared handler for every method: log, then a 200 with a short body."""
        LOG.write(self.client_address[0], sport=self.client_address[1], method=self.command,
                  path=self.path, headers=dict(self.headers.items()))
        body = b"ok\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_GET = do_POST = do_HEAD = handle_any

    def log_message(self, *args):
        """Silences the default stderr access log (the JSON log has everything)."""


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 80), Canary).serve_forever()
