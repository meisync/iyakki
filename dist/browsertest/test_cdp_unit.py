# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Unit tests of the DevTools client against a fake DevTools server (no Chrome): routing of answers and events, errors, timeouts."""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cdp  # noqa: E402
import test_ws  # noqa: E402
import ws  # noqa: E402


def devtools_script(c):
    buf = b""
    while True:
        (fin, op, payload, _), buf = test_ws.read_client_frame(c, buf)
        if op == ws.OP_CLOSE:
            return
        m = json.loads(payload)
        send = lambda d: c.sendall(ws.encode_frame(ws.OP_TEXT, json.dumps(d).encode(), mask=False))
        meth = m["method"]
        if meth == "Test.echo":
            send({"id": m["id"], "result": {"echo": m["params"], "session": m.get("sessionId")}})
        elif meth == "Test.fail":
            send({"id": m["id"], "error": {"code": -32000, "message": "nope"}})
        elif meth == "Test.events":
            for ev in ({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"value": "boom"}, {"value": 7}]}},
                       {"method": "Runtime.consoleAPICalled", "params": {"type": "log", "args": [{"value": "fine"}]}},
                       {"method": "Runtime.exceptionThrown", "params": {"exceptionDetails": {"text": "x", "exception": {"description": "TypeError: y"}}}},
                       {"method": "Network.responseReceived", "params": {"response": {"url": "https://h/a.js", "status": 404}}},
                       {"method": "Network.responseReceived", "params": {"response": {"url": "https://h/b.js", "status": 200}}},
                       {"method": "Log.entryAdded", "params": {"entry": {"level": "warning", "text": "w", "source": "security"}}}):
                ev["sessionId"] = "S1"
                send(ev)
            send({"id": m["id"], "result": {}})
        elif meth == "Test.eval":
            send({"id": m["id"], "result": {"result": {"type": "number", "value": 3}}})
        elif meth == "Test.silent":
            pass


class Client(unittest.TestCase):
    def setUp(self):
        port, self.t = test_ws.fake_server(devtools_script)
        self.conn = cdp.Connection(ws.WebSocket.connect("127.0.0.1", port, "/devtools/browser/x"))

    def tearDown(self):
        self.conn.close()

    def test_answers_are_routed_by_id_and_session(self):
        r = self.conn.call("Test.echo", {"a": 1}, session="S1")
        self.assertEqual(r, {"echo": {"a": 1}, "session": "S1"})

    def test_protocol_errors_and_timeouts(self):
        with self.assertRaises(cdp.CdpError):
            self.conn.call("Test.fail")
        with self.assertRaises(cdp.CdpError):
            self.conn.call("Test.silent", timeout=0.3)

    def test_page_collects_console_errors_and_failed_requests(self):
        p = cdp.Page(self.conn, "T1", "S1")
        self.conn.call("Test.events", session="S1")
        import time
        time.sleep(0.2)
        errs = p.errors()
        self.assertIn("boom 7", errs)
        self.assertIn("uncaught: TypeError: y", errs)
        self.assertIn("HTTP 404 https://h/a.js", errs)
        self.assertEqual(len(errs), 3, errs)
        self.assertEqual(p.errors(ignore=["boom"]), errs[1:])
        p.clear_errors()
        self.assertEqual(p.errors(), [])

    def test_evaluate_returns_the_value(self):
        p = cdp.Page(self.conn, "T1", "S1")
        self.assertEqual(p.call("Test.eval") ["result"]["value"], 3)

    def test_a_closed_connection_wakes_the_caller(self):
        self.conn.ws.sock.close()
        with self.assertRaises(Exception):
            self.conn.call("Test.silent", timeout=3)


class FindingChrome(unittest.TestCase):
    def fake(self, d, text):
        p = os.path.join(d, "fake-chrome")
        open(p, "w").write("#!/bin/sh\necho '%s'\n" % text)
        os.chmod(p, 0o755)
        return p

    def test_an_explicit_choice_is_used_or_nothing_is_found(self):
        import tempfile
        old = os.environ.get("IYAKKI_CHROME")
        try:
            with tempfile.TemporaryDirectory() as d:
                os.environ["IYAKKI_CHROME"] = os.path.join(d, "nope")
                self.assertIsNone(cdp.find_chrome(), "a wrong explicit path is not replaced by a guess")
                p = self.fake(d, "Chromium 140.0.1.2 snap")
                os.environ["IYAKKI_CHROME"] = p
                self.assertEqual(cdp.find_chrome(), p)
                self.assertEqual(cdp.chrome_major(p), 140)
                self.assertEqual(cdp.chrome_major(self.fake(d, "garbage")), 0)
                self.assertEqual(cdp.chrome_major(os.path.join(d, "missing")), 0)
        finally:
            if old is None:
                os.environ.pop("IYAKKI_CHROME", None)
            else:
                os.environ["IYAKKI_CHROME"] = old

    def test_the_runner_says_nothing_was_tested_and_exits_2(self):
        import subprocess
        import tempfile
        here = os.path.dirname(os.path.abspath(__file__))
        with tempfile.TemporaryDirectory() as d:
            for chrome, word in ((os.path.join(d, "none"), "no Chrome"), (self.fake(d, "Chromium 90.0.1.1"), "version")):
                r = subprocess.run([sys.executable, "-B", os.path.join(here, "run_pages.py"), "--out", os.path.join(d, "out")], env=dict(os.environ, IYAKKI_CHROME=chrome), capture_output=True, text=True, timeout=60)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertIn("NOTHING WAS TESTED", r.stdout)
                self.assertIn(word, r.stdout)


if __name__ == "__main__":
    unittest.main()
