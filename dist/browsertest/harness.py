# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Shared parts of the real-browser suites: the result report, the web service fixture (the real service on a loopback TLS socket,
started by the ignored Rust test `browser_fixture_server`), and a small static file server for pages that need no service."""
import http.server
import json
import os
import re
import signal
import subprocess
import threading
import time
import traceback


class Report:
    """Collects PASS/FAIL lines with evidence; writes results.json and a text log in the output folder."""

    def __init__(self, outdir):
        self.out = outdir
        os.makedirs(outdir, exist_ok=True)
        self.rows = []
        self.section = ""

    def shot(self, name):
        return os.path.join(self.out, name)

    def check(self, name, cond, detail="", known=None):
        """`known` names a KNOWN-ISSUES entry that explains a failure we have not fixed: the row is recorded as KNOWN (it does not fail the
        run), and a check that passes although it is marked known says so, so the mark gets removed."""
        if known and not cond:
            self.rows.append({"section": self.section, "name": name, "ok": True, "known": known, "detail": str(detail)[:2000]})
            print("KNOWN %s [%s] %s%s" % (known, self.section, name, "" if not detail else "  -- " + str(detail)[:300]), flush=True)
            return False
        if known and cond:
            print("NOTE  %s is fixed: remove known=%r from [%s] %s" % (known, known, self.section, name), flush=True)
        self.rows.append({"section": self.section, "name": name, "ok": bool(cond), "detail": str(detail)[:2000]})
        print(("PASS " if cond else "FAIL ") + "[%s] %s%s" % (self.section, name, "" if cond or not detail else "  -- " + str(detail)[:400]), flush=True)
        return bool(cond)

    def metric(self, name, values):
        """A measurement (sizes, times) recorded in results.json; never a pass or a fail by itself."""
        self.rows.append({"section": self.section, "name": name, "ok": True, "info": True, "metrics": values, "detail": json.dumps(values)[:500]})
        print("METRIC [%s] %s %s" % (self.section, name, json.dumps(values)[:300]), flush=True)

    def info(self, name, detail):
        self.rows.append({"section": self.section, "name": name, "ok": True, "info": True, "detail": str(detail)[:2000]})
        print("INFO [%s] %s  %s" % (self.section, name, str(detail)[:300]), flush=True)

    def run(self, section, fn, *a):
        """Run one suite function; an exception is a FAIL with its traceback, never a crash of the whole run."""
        self.section = section
        t0 = time.time()
        try:
            fn(self, *a)
        except Exception:
            self.check("suite ran to the end", False, traceback.format_exc())
        print("---- %s took %.1fs" % (section, time.time() - t0), flush=True)

    def summary(self):
        real = [r for r in self.rows if not r.get("info")]
        known = [r for r in real if r.get("known")]
        if known:
            print("\n%d known issue(s) not counted as failures:" % len(known))
            for r in known:
                print("  %s [%s] %s" % (r["known"], r["section"], r["name"]))
        bad = [r for r in real if not r["ok"]]
        with open(os.path.join(self.out, "results.json"), "w") as f:
            json.dump(self.rows, f, indent=1)
        print("\n%d checks, %d failed" % (len(real), len(bad)))
        for r in bad:
            print("  FAILED [%s] %s: %s" % (r["section"], r["name"], r["detail"][:300]))
        return len(bad)


class Fixture:
    """The real web service. `binary` is the compiled iyakki-agent lib test binary; the ignored test inside starts the service and
    waits for the stop file. Kills the process group at the end whatever happened."""

    def __init__(self, binary, work, profile="cloud", test="webservice::tests::browser_fixture_server", env=None):
        self.binary, self.profile, self.test = binary, profile, test
        self.env_extra = dict(env or {})
        self.work = work
        os.makedirs(work, exist_ok=True)
        self.info_path = os.path.join(work, "fixture-%s.json" % profile)
        self.proc = None
        self.info = None

    def start(self, timeout=60):
        for p in (self.info_path, self.info_path + ".stop", self.info_path + ".turns"):
            if os.path.exists(p):
                os.remove(p)
        env = dict(os.environ, IYAKKI_BROWSERTEST_INFO=self.info_path, IYAKKI_BROWSERTEST_PROFILE=self.profile, **self.env_extra)
        log = open(os.path.join(self.work, "fixture-%s.log" % self.profile), "wb")
        self.proc = subprocess.Popen([self.binary, "--ignored", "--exact", self.test, "--nocapture", "--test-threads", "1"], env=env, stdout=log, stderr=log, stdin=subprocess.DEVNULL, start_new_session=True)
        t0 = time.time()
        while time.time() - t0 < timeout:
            if self.proc.poll() is not None:
                raise RuntimeError("the fixture exited early (see fixture-%s.log)" % self.profile)
            if os.path.exists(self.info_path):
                self.info = json.load(open(self.info_path))
                return self
            time.sleep(0.1)
        self.stop()
        raise RuntimeError("the fixture did not start")

    @property
    def base(self):
        return "https://127.0.0.1:%d" % self.info["port"]

    def turn(self, line):
        """Queue the model's next turn: `call` (asks to fetch a page), `err`, `hang`, or `final:TEXT`."""
        with open(self.info_path + ".turns", "a") as f:
            f.write(line + "\n")
        time.sleep(0.4)  # the fixture reads the file ten times a second

    def stop(self):
        p, self.proc = self.proc, None
        if p is None:
            return
        try:
            open(self.info_path + ".stop", "w").close()
            p.wait(5)
        except Exception:
            pass
        if p.poll() is None:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.killpg(p.pid, sig)
                except (ProcessLookupError, PermissionError):
                    break
                try:
                    p.wait(5)
                    break
                except subprocess.TimeoutExpired:
                    pass

    def __enter__(self):
        return self.start()

    def __exit__(self, *_):
        self.stop()


class Static:
    """Serves some directories read-only on 127.0.0.1 (no Content-Security-Policy, so test pages may run inline code).
    `mounts` maps a URL prefix ("/ui/") to a directory; `extra` maps an exact path to (content type, bytes)."""

    def __init__(self, mounts, extra=None):
        mounts = dict(mounts)
        extra = extra if extra is not None else {}  # not copied: pages may be added while the server runs
        self.extra = extra
        types = {".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".html": "text/html", ".json": "application/json", ".wasm": "application/wasm", ".bin": "application/octet-stream", ".rgba": "application/octet-stream", ".txt": "text/plain"}

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                path = self.path.split("?", 1)[0]
                body, ctype = None, "application/octet-stream"
                if path == "/favicon.ico":
                    self.send_response(204)
                    self.end_headers()
                    return
                if path in extra:
                    ctype, body = extra[path]
                else:
                    for pre, d in mounts.items():
                        if path.startswith(pre) and ".." not in path:
                            f = os.path.join(d, path[len(pre):])
                            if os.path.isfile(f):
                                body = open(f, "rb").read()
                                ctype = types.get(os.path.splitext(f)[1], ctype)
                                break
                if body is None:
                    self.send_response(404)
                    self.end_headers()
                    return
                self.send_response(200)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(body)

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.srv.daemon_threads = True
        self.t = threading.Thread(target=self.srv.serve_forever, daemon=True)
        self.t.start()

    @property
    def base(self):
        return "http://127.0.0.1:%d" % self.srv.server_address[1]

    def stop(self):
        self.srv.shutdown()
        self.srv.server_close()


def cargo_executables(text):
    """Paths of test executables from `cargo test --no-run` output: {'lib-name or test-name': path}."""
    out = {}
    for m in re.finditer(r"Executable (?:unittests (\S+)|(tests/\S+)) \((.+?)\)", text):
        out[m.group(1) or m.group(2)] = m.group(3)
    return out
