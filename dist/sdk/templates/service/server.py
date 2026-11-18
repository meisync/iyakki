"""__TITLE__: a small HTTP service (standard library only). GET /healthz answers ok; GET / answers a greeting."""
import http.server
import os
import signal
import sys


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/healthz":
            body = b"ok\n"
        elif self.path == "/":
            body = b"Hello from __NAME__\n"
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        sys.stdout.write("%s\n" % (fmt % args))
        sys.stdout.flush()


def make_server(port):
    return http.server.ThreadingHTTPServer(("0.0.0.0", port), Handler)


if __name__ == "__main__":
    srv = make_server(int(os.environ.get("PORT", "8080")))
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(SystemExit(0)))
    try:
        srv.serve_forever()
    except (KeyboardInterrupt, SystemExit):
        pass
