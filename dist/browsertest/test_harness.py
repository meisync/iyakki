# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Unit tests of the harness parts that need no browser and no service."""
import contextlib
import io
import os
import sys
import tempfile
import unittest
import urllib.request
import urllib.error

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import harness  # noqa: E402


class Parts(unittest.TestCase):
    def test_report_counts_and_writes(self):
        with tempfile.TemporaryDirectory() as d:
            r = harness.Report(d)
            r.section = "s"
            r.check("a", True)
            r.check("b", False, "why")
            r.info("i", "x")
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(r.summary(), 1)
            self.assertTrue(os.path.exists(os.path.join(d, "results.json")))

    def test_a_crashing_suite_becomes_a_failure(self):
        with tempfile.TemporaryDirectory() as d:
            r = harness.Report(d)
            with contextlib.redirect_stdout(io.StringIO()):
                r.run("boom", lambda rep: 1 / 0)
                self.assertEqual(r.summary(), 1)

    def test_cargo_output_is_parsed(self):
        t = "  Executable unittests src/lib.rs (/t/debug/deps/iyakki_agent-abc)\n  Executable tests/webui_tiles_js.rs (/t/debug/deps/webui_tiles_js-def)\n"
        self.assertEqual(harness.cargo_executables(t), {"src/lib.rs": "/t/debug/deps/iyakki_agent-abc", "tests/webui_tiles_js.rs": "/t/debug/deps/webui_tiles_js-def"})

    def test_static_server_serves_mounts_only(self):
        with tempfile.TemporaryDirectory() as d:
            open(os.path.join(d, "a.js"), "w").write("1")
            s = harness.Static({"/ui/": d}, {"/x": ("text/plain", b"hi")})
            try:
                self.assertEqual(urllib.request.urlopen(s.base + "/ui/a.js").read(), b"1")
                self.assertEqual(urllib.request.urlopen(s.base + "/x").read(), b"hi")
                for bad in ("/ui/../etc/passwd", "/ui/nope.js", "/other"):
                    with self.assertRaises(urllib.error.HTTPError):
                        urllib.request.urlopen(s.base + bad)
            finally:
                s.stop()


if __name__ == "__main__":
    unittest.main()
