"""Static checks of the tool project: manifest fields, capabilities in the allowed vocabulary, the module's exports.
Behaviour is tested by running the module: `iyakki run module.wat --entry run` or the unit tests of your own language."""
import json
import os
import re
import unittest

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class ToolTest(unittest.TestCase):
    def setUp(self):
        with open(os.path.join(HERE, "tool.json"), encoding="utf-8") as f:
            self.m = json.load(f)
        with open(os.path.join(HERE, "module.wat"), encoding="utf-8") as f:
            self.wat = f.read()

    def test_manifest(self):
        for k in ("name", "version", "license", "description", "schema"):
            self.assertIn(k, self.m)
        self.assertRegex(self.m["name"], r"^[a-z0-9][a-z0-9-]{0,47}$")
        self.assertEqual(self.m["schema"]["type"], "object")
        self.assertLessEqual(self.m.get("budget", {}).get("seconds", 5), 30)

    def test_capabilities_are_narrow(self):
        for c in self.m.get("capabilities", []):
            self.assertRegex(c, r"^(read|write):/[^*]+$|^net:[a-z0-9.-]+$", "name the folder or the host, no wildcards")

    def test_project_matches_manifest(self):
        with open(os.path.join(HERE, "iyakki-app.json"), encoding="utf-8") as f:
            app = json.load(f)
        self.assertEqual((app["name"], app["version"]), (self.m["name"], self.m["version"]))
        self.assertEqual(sorted(app["permissions"]), sorted(self.m["capabilities"]))

    def test_module_exports(self):
        self.assertIn("(module", self.wat)
        self.assertTrue(re.search(r'\(export "run"', self.wat) and re.search(r'\(export "memory"', self.wat))


if __name__ == "__main__":
    unittest.main()
