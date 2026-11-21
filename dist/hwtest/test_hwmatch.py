"""Unit tests of the NOT-APPLICABLE rule: a recipe written for specific hardware is skipped, never FAIL, on another machine.
No device: a fake Ctx returns canned `dev info` text.

    python3 -I -m unittest discover -s dist/hwtest -p 'test_hwmatch.py'
"""
import os, sys, unittest
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hwcheck, hwmatch
from hwcheck import Verdict, RECIPES

LENOVO = "iyakki\n  device     LENOVO 82WM Legion Pro 5 16ARX8\n  image      IYAKKI 0.1.0-alpha.7-dev\n"
M14 = "iyakki\n  device     Apple Inc. Macmini7,1 1.0\n"
M18 = "iyakki\n  device     Apple Inc. Macmini8,1 1.0\n"
NO_DEVICE = "iyakki\n  image      IYAKKI 0.1.0\n"


class FakeCtx(hwcheck.Ctx):
    def __init__(self, info):
        self.rid, self.dir, self.n, self.touched, self.lines, self.info = "T", "/nonexistent", 0, [], [], info
    def log(self, s): self.lines.append(s)
    def run(self, *a, name=None, timeout=60): return 0, self.info


def outcome(f, *a):
    try:
        f(*a); return ("PASS", "")
    except Verdict as e:
        return (e.kind, e.reason)


class Rule(unittest.TestCase):
    def test_match_and_mismatch(self):
        self.assertEqual(hwmatch.check("macmini2014", M14), (True, ""))
        ok, why = hwmatch.check("macmini2014", LENOVO)
        self.assertFalse(ok); self.assertIn("Macmini7,1", why); self.assertIn("LENOVO 82WM", why)
        self.assertFalse(hwmatch.check("macmini2018", M14)[0])
        self.assertTrue(hwmatch.check("macmini2018", M18)[0])
        self.assertTrue(hwmatch.check("apple", M14)[0]); self.assertFalse(hwmatch.check("apple", LENOVO)[0])

    def test_unknown_device_is_not_a_match_with_a_reason(self):
        ok, why = hwmatch.check("macmini2014", NO_DEVICE)
        self.assertFalse(ok); self.assertIn("cannot be identified", why)

    def test_require_hardware_gives_not_applicable_not_fail(self):
        k, why = outcome(hwcheck.require_hardware, FakeCtx(LENOVO), "macmini2014")
        self.assertEqual(k, "NOT-APPLICABLE"); self.assertIn("Macmini7,1", why)
        self.assertEqual(outcome(hwcheck.require_hardware, FakeCtx(M14), "macmini2014")[0], "PASS")

    def test_every_mac_mini_2014_recipe_is_skipped_elsewhere(self):
        hwcheck.from_recipes()
        import recipes5
        ids = [i for i, (_, f) in RECIPES.items() if f.__module__ == "recipes5"]
        self.assertGreater(len(ids), 30)
        for i in ids:
            c = FakeCtx(LENOVO)
            kind, _ = outcome(RECIPES[i][1], c)
            self.assertEqual(kind, "NOT-APPLICABLE", i)

    def test_mac_only_recipes_skip_on_a_pc(self):
        hwcheck.from_recipes()
        for i in ("F-DEV-02", "F-HW-05", "F-DRIVERS-07", "F-DRIVERS-30"):
            self.assertEqual(outcome(RECIPES[i][1], FakeCtx(LENOVO))[0], "NOT-APPLICABLE", i)

    def test_run_row_prints_and_records_not_applicable(self):
        hwcheck.from_recipes()
        import tempfile, io, contextlib
        with tempfile.TemporaryDirectory() as d:
            old = (hwcheck.ROOT, hwcheck.BIN)
            hwcheck.ROOT = d
            hwcheck.recipe("F-TEST-NA", "x")(lambda c: hwcheck.not_applicable("no such hardware"))
            hwcheck.Ctx.home = lambda self: None
            try:
                buf = io.StringIO()
                with contextlib.redirect_stdout(buf):
                    v = hwcheck.run_row("F-TEST-NA")
            finally:
                hwcheck.ROOT, hwcheck.BIN = old; RECIPES.pop("F-TEST-NA")
            self.assertEqual(v, ("NOT-APPLICABLE", "no such hardware"))
            self.assertIn("NOT-APPLICABLE", buf.getvalue())

    def test_nvidia_and_startup_facts(self):
        self.assertTrue(hwmatch.nvidia_card_present("[ 2.7] nouveau 0000:01:00.0: NVIDIA AD106 (196000a1)"))
        self.assertFalse(hwmatch.nvidia_card_present(""))
        self.assertTrue(hwmatch.startup_item_absent(["Touch screen", "off", "not on this machine"], "Touch screen"))
        self.assertFalse(hwmatch.startup_item_absent(["Touch screen", "ready"], "Touch screen"))


WHOAMI_ALL = "Scopes this computer holds: debug, files:system\nScopes (an administrator grants more on the device):\n  [x] debug           read device info and logs for the test\n  [x] files:system    read (never write) /etc and the data folder\n  [ ] files           copy files to and from the account's home folder\n"


class Scopes(unittest.TestCase):
    def test_holds_scope(self):
        import recipes2
        self.assertTrue(recipes2.holds_scope(WHOAMI_ALL, "files:system"))
        self.assertFalse(recipes2.holds_scope(WHOAMI_ALL, "files"))
        self.assertFalse(recipes2.holds_scope(WHOAMI_ALL, "kube"))

    def test_devmode12_follows_the_held_scope_not_a_fixed_expectation(self):
        hwcheck.from_recipes()
        import tempfile
        inbox = "3 notifications, 1 unread\nby kind: info 3\nby source: x 3\n"
        class C(FakeCtx):
            def __init__(self, held, bodies_rc, d):
                super().__init__(""); self.held, self.rc, self.dir = held, bodies_rc, d
            def run(self, *a, name=None, timeout=60):
                if a[0] == "whoami": return 0, ("  [x] files:system  read it, never write it\n" if self.held else "  [ ] files:system  read it, never write it\n")
                if a[:2] == ("inbox", "--bodies"): return self.rc, "needs the files:system scope" if self.rc else "bodies"
                return 0, inbox
        with tempfile.TemporaryDirectory() as d:
            f = RECIPES["F-DEVMODE-12"][1]
            def fake_dev(*a, **k):
                if a and a[0] == "diag": open(a[2], "w").write("version and machine\nmemory\ndisks and mounts\nnetwork\nservices\ncluster\naudit\nsound\n")
                return 0, ""
            old_dev = hwcheck.dev; self.addCleanup(setattr, hwcheck, 'dev', old_dev)
            import recipes2 as _r2; self.addCleanup(setattr, _r2, 'dev', _r2.dev)
            hwcheck.dev = fake_dev
            import recipes2; recipes2.dev = hwcheck.dev
            for held, rc in ((True, 0), (False, 1)):
                self.assertEqual(outcome(f, C(held, rc, d))[0], "PASS", (held, rc))
            # held but refused, and not held but answered, are the real failures
            self.assertIn("is held but --bodies was refused", outcome(f, C(True, 1, d))[1])
            self.assertIn("worked without the files:system scope", outcome(f, C(False, 0, d))[1])


if __name__ == "__main__":
    unittest.main()
