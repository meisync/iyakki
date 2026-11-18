"""Tests with a fake client: it speaks JSON-RPC to the real server process over pipes, as the MCP tile would."""
import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, HERE)
import server  # noqa: E402


class FakeClient:
    def __init__(self):
        self.p = subprocess.Popen([sys.executable, os.path.join(HERE, "server.py")], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.n = 0

    def call(self, method, params=None):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=5)


class ServerTest(unittest.TestCase):
    def setUp(self):
        self.c = FakeClient()
        self.addCleanup(self.c.close)

    def test_handshake_and_listing(self):
        r = self.c.call("initialize", {"protocolVersion": "2024-11-05"})["result"]
        self.assertEqual(r["protocolVersion"], server.PROTOCOL)
        names = [t["name"] for t in self.c.call("tools/list")["result"]["tools"]]
        self.assertEqual(sorted(names), sorted(server.TOOLS))

    def test_tool_calls(self):
        r = self.c.call("tools/call", {"name": "add", "arguments": {"a": 2, "b": 3}})["result"]
        self.assertEqual(r["content"][0]["text"], "5")
        r = self.c.call("tools/call", {"name": "echo", "arguments": {"text": "hi", "shout": True}})["result"]
        self.assertEqual(r["content"][0]["text"], "HI")

    def test_bad_input_is_refused_before_the_tool_runs(self):
        for args in ({}, {"a": "x", "b": 1}, {"a": 1, "b": 2, "c": 3}, {"a": True, "b": 1}):
            self.assertIn("error", self.c.call("tools/call", {"name": "add", "arguments": args}))
        self.assertIn("error", self.c.call("tools/call", {"name": "echo", "arguments": {"text": "x" * 5000}}))
        self.assertIn("error", self.c.call("tools/call", {"name": "nope", "arguments": {}}))

    def test_resources_and_prompts(self):
        uri = self.c.call("resources/list")["result"]["resources"][0]["uri"]
        self.assertIn("text", self.c.call("resources/read", {"uri": uri})["result"]["contents"][0])
        m = self.c.call("prompts/get", {"name": "summarise", "arguments": {"text": "abc"}})["result"]["messages"]
        self.assertIn("abc", m[0]["content"]["text"])

    def test_garbage_does_not_kill_the_server(self):
        self.c.p.stdin.write("not json\n")
        self.c.p.stdin.flush()
        self.assertEqual(json.loads(self.c.p.stdout.readline())["error"]["code"], -32700)
        self.assertEqual(self.c.call("ping")["result"], {})
        self.assertEqual(self.c.call("no/such/method")["error"]["code"], -32601)

    def test_a_slow_tool_is_cut_off(self):
        import time
        old = server.TOOL_SECONDS
        server.TOOL_SECONDS = 0.2
        try:
            with self.assertRaises(TimeoutError):
                server.run_bounded(lambda a: time.sleep(5), {})
        finally:
            server.TOOL_SECONDS = old

    def test_no_secrets_or_shell_in_the_source(self):
        src = open(os.path.join(HERE, "server.py"), encoding="utf-8").read()
        for bad in ("os.system", "shell=True", "ghp_", "sk-ant-"):
            self.assertNotIn(bad, src)


if __name__ == "__main__":
    unittest.main()
