"""canary.py - leak lab canary web server. Stands in for the RSS feed, the search provider and the
update check (the lab dns points api.github.com here), and for the bait tracker's redirect target.
:80  logs the source IP, method, path and every request header, then answers 200: an empty RSS
     feed for paths starting /rss or /search (so the app's feed parser is happy), else "ok".
:443 logs the source of every TCP connection and closes it (the update check speaks TLS to
     GitHub; the lab only needs to know which path the connection took)."""
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from lablog import LabLog

LOG = LabLog("canary")
RSS = (b'<?xml version="1.0"?><rss version="2.0"><channel><title>lab</title>'
       b'<item><title>lab item</title><guid>lab-1</guid><link>http://10.88.0.24/lab-1.torrent</link></item>'
       b'</channel></rss>\n')


class Canary(BaseHTTPRequestHandler):
    """Logs any request and answers 200."""

    def handle_any(self):
        """Shared handler for every method: log, then a 200 with an RSS or a short body."""
        LOG.write(self.client_address[0], sport=self.client_address[1], port=80, method=self.command,
                  path=self.path, headers=dict(self.headers.items()))
        body = RSS if self.path.startswith(("/rss", "/search")) else b"ok\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_GET = do_POST = do_HEAD = handle_any

    def log_message(self, *args):
        """Silences the default stderr access log (the JSON log has everything)."""


def tls_logger():
    """:443 accept loop: logs each connection's source, then closes it."""
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", 443))
    s.listen(16)
    while True:
        conn, (ip, port) = s.accept()
        LOG.write(ip, sport=port, port=443, note="TCP connection (TLS not served)")
        conn.close()


if __name__ == "__main__":
    threading.Thread(target=tls_logger, daemon=True).start()
    ThreadingHTTPServer(("0.0.0.0", 80), Canary).serve_forever()
