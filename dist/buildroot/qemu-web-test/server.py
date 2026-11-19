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
        log.write("GET " + self.path + (" " + self.headers["Range"] if self.headers.get("Range") else "") + "\n"); log.flush()
        # byte ranges: the engine's GStreamer HTTP source asks for them and treats a plain 200 as an error (mp4 files)
        rng = self.headers.get("Range")
        fs = self.translate_path(self.path)
        if rng and rng.startswith("bytes=") and os.path.isfile(fs):
            size = os.path.getsize(fs)
            a, _, b = rng[6:].partition("-")
            try:
                start = int(a) if a else max(0, size - int(b)); end = int(b) if (a and b) else size - 1
            except ValueError:
                start, end = 0, size - 1
            end = min(end, size - 1)
            if start > end:
                self.send_response(416); self.send_header("Content-Range", "bytes */%d" % size); self.end_headers(); return
            self.send_response(206)
            self.send_header("Content-Type", self.guess_type(fs)); self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Range", "bytes %d-%d/%d" % (start, end, size)); self.send_header("Content-Length", str(end - start + 1)); self.end_headers()
            with open(fs, "rb") as f:
                f.seek(start); self.wfile.write(f.read(end - start + 1))
            return
        super().do_GET()
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("0.0.0.0", port), H).serve_forever()
