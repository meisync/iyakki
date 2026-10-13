#!/usr/bin/env python3
"""Serves the full-browser test page and logs what the page reports about itself (canvas fps, video state)."""
import http.server, os, sys, urllib.parse, json
root = os.path.dirname(os.path.abspath(__file__))
port = int(sys.argv[1]) if len(sys.argv) > 1 else 8731
log = open(sys.argv[2], "a") if len(sys.argv) > 2 else sys.stderr
class H(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k): super().__init__(*a, directory=root, **k)
    def do_GET(self):
        p = urllib.parse.urlparse(self.path)
        if p.path == "/report":
            log.write("REPORT " + urllib.parse.unquote(p.query) + "\n"); log.flush()
            self.send_response(204); self.end_headers(); return
        log.write("GET " + self.path + "\n"); log.flush()
        super().do_GET()
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("0.0.0.0", port), H).serve_forever()
