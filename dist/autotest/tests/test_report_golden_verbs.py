import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import evidence_report as ER  # noqa: E402
import golden  # noqa: E402
import results as R  # noqa: E402
import verbs  # noqa: E402
import devui  # noqa: E402
from test_evidence import FEATURE, Fix, features  # noqa: E402

try:
    from PIL import Image
except ImportError:  # pragma: no cover
    Image = None


class Report(unittest.TestCase):
    def test_levels_are_listed_apart_and_a_row_without_machine_evidence_is_not_listed(self):
        f = Fix(self)
        f.add("run1")
        f.add("run2", target="real", device="lenovo", when="2026-11-22T04:00:00")
        os.makedirs(os.path.join(f.out, "human"))
        with open(os.path.join(f.out, "human", "F-T-1.c1.lenovo.json"), "w") as fh:
            json.dump({"row": "F-T-1", "clause": "c1", "machine": "lenovo", "verdict": "pass", "date": "2026-11-22"}, fh)
        per = ER.collect(features(), [f.out])
        lv = per["F-T-1"]["levels"]
        self.assertEqual(set(lv), {"unit", "qemu", "machine-lenovo", "human"})
        self.assertEqual(lv["unit"]["tests"], 1)
        md = ER.render_md(per)
        self.assertIn("qemu: PASS", md)
        self.assertIn("machine-lenovo: PASS", md)
        self.assertIn("human: c1/lenovo pass", md)
        self.assertIn("0 of 1", ER.render_md(ER.collect(features(), [])))
        self.assertEqual(ER.render_md(per), ER.render_md(ER.collect(features(), [f.out])), "deterministic")


@unittest.skipUnless(Image, "no Pillow")
class Golden(unittest.TestCase):
    def pic(self, d, name, fill=(10, 20, 30), box=None, colour=(250, 250, 250)):
        im = Image.new("RGB", (100, 50), fill)
        if box:
            im.paste(colour, box)
        p = os.path.join(d, name)
        im.save(p)
        return p

    def test_identical_passes_a_clock_change_is_masked_and_a_real_change_fails(self):
        with tempfile.TemporaryDirectory() as d:
            g = self.pic(d, "g.png")
            self.assertTrue(golden.compare(g, self.pic(d, "a.png"))["ok"])
            clock = self.pic(d, "c.png", box=(80, 0, 100, 10))
            self.assertFalse(golden.compare(g, clock)["ok"])
            self.assertTrue(golden.compare(g, clock, masks=[(80, 0, 20, 10)])["ok"])
            big = self.pic(d, "b.png", box=(0, 0, 50, 50))
            r = golden.compare(g, big, masks=[(80, 0, 20, 10)])
            self.assertFalse(r["ok"])
            self.assertGreater(r["changed_percent"], 40)

    def test_small_noise_under_the_tolerance_and_a_different_size(self):
        with tempfile.TemporaryDirectory() as d:
            g = self.pic(d, "g.png")
            self.assertTrue(golden.compare(g, self.pic(d, "n.png", fill=(20, 30, 40)))["ok"])
            small = os.path.join(d, "s.png")
            Image.new("RGB", (10, 10)).save(small)
            self.assertFalse(golden.compare(g, small)["size_ok"])

    def test_names_and_masks_are_checked(self):
        with self.assertRaises(ValueError):
            golden.golden_path("/g", "F-A-1", "../x")
        with self.assertRaises(ValueError):
            golden.parse_masks(["mask=1,2"])
        self.assertEqual(golden.parse_masks(["mask=1,2,3,4"]), [(1, 2, 3, 4)])


class FakeT:
    """The part of devui.Test the verbs use."""

    def __init__(self, shots=(), dev=None):
        self.checks, self.notes, self.shots, self.dev = [], [], list(shots), dev

    def check(self, name, ok, detail=""):
        self.checks.append((name, bool(ok), detail))

    def note(self, x):
        self.notes.append(x)

    def skipped_check(self, name, why):
        self.checks.append((name, True, "skipped"))

    def shot(self, label, st=None):
        raise AssertionError("no new shot expected")


class FakeDev:
    def __init__(self, logs="", cfg=""):
        self.logs, self.cfg = logs, cfg

    def info(self):
        return "compat   peer computer x (developer mode)\n"

    def run(self, args):
        return self.logs

    def config_get(self, k):
        return self.cfg


class Verbs(unittest.TestCase):
    def test_audit_serial_sh_config(self):
        t = FakeT(dev=FakeDev("a\nchat.form.submitted n=2\n", "display.size = Large"))
        verbs.v_expect_audit(t, r"chat\.form\.submitted")
        verbs.v_expect_audit(t, r"nothing like this")
        verbs.v_expect_info(t, r"peer computer .*developer mode")
        verbs.v_expect_info(t, r"open session")
        verbs.v_expect_config(t, "display.size Large")
        verbs.v_expect_config(t, "display.size Small")
        self.assertEqual([c[1] for c in t.checks], [True, False, True, False, True, False])
        verbs.Active.reset("F-T-1", serial=lambda: "boot ok\nsupervisor up", sh=lambda c: (0, "hello world"))
        verbs.v_expect_serial(t, "supervisor up")
        verbs.v_expect_sh(t, "echo hi ==> hello")
        verbs.v_expect_sh(t, "echo hi ==> absent")
        self.assertEqual([c[1] for c in t.checks][6:], [True, True, False])
        verbs.Active.reset("F-T-1")
        with self.assertRaises(devui.Skip):
            verbs.v_expect_serial(t, "x")
        with self.assertRaises(devui.Skip):
            verbs.v_expect_sh(t, "x ==> y")
        with self.assertRaises(devui.CheckFailed):
            verbs.Active.sh = lambda c: (0, "")
            verbs.v_expect_sh(t, "no separator")

    def test_http_only_talks_to_loopback(self):
        verbs.Active.reset("F-T-1")
        with self.assertRaises(devui.CheckFailed):
            verbs.v_expect_http(FakeT(), "http://example.com/ 200")
        with self.assertRaises(devui.Skip):
            verbs.v_expect_http(FakeT(), "{http}/healthz 200")

    @unittest.skipUnless(Image, "no Pillow")
    def test_golden_verb_new_then_pass_then_fail(self):
        with tempfile.TemporaryDirectory() as d:
            png = os.path.join(d, "01-home.png")
            Image.new("RGB", (100, 50), (5, 5, 5)).save(png)
            shot = devui.Shot(png, "home", 1)
            verbs.Active.reset("F-T-1", golden_root=os.path.join(d, "golden"))
            t = FakeT([shot])
            verbs.v_expect_golden(t, "home")
            self.assertEqual(len(verbs.Active.new_goldens), 1)
            self.assertFalse(t.checks, "a missing golden is a candidate, not a check that passes")
            verbs.Active.reset("F-T-1", golden_root=os.path.join(d, "golden"), update_golden=True)
            verbs.v_expect_golden(t, "home")
            self.assertTrue(os.path.isfile(os.path.join(d, "golden", "F-T-1", "home.png")))
            verbs.Active.reset("F-T-1", golden_root=os.path.join(d, "golden"))
            verbs.v_expect_golden(t, "home")
            self.assertEqual(t.checks[-1][1], True)
            Image.new("RGB", (100, 50), (250, 250, 250)).save(png)
            verbs.v_expect_golden(t, "home")
            self.assertEqual(t.checks[-1][1], False)
            self.assertEqual(len(verbs.Active.golden_diffs), 1)


if __name__ == "__main__":
    unittest.main()
