"""run_entries on a fake `iyakki dev` (the real Device, guard and scenario reader; only `run` is canned), then the evidence rules on what it wrote."""
import datetime
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import devui  # noqa: E402
import evidence  # noqa: E402
import results as R  # noqa: E402
import run  # noqa: E402
import verbs  # noqa: E402

try:
    from PIL import Image, ImageDraw
except ImportError:  # pragma: no cover
    Image = None

FEATURE = "Settings has a Firewall page that is drawn"
STATE = {"app": "Home", "page": "", "focused": "", "focused_secret": False, "cut": False, "screen": [320, 200],
         "lines": [{"text": "IYAKKI", "x": 10, "y": 5}, {"text": "choose a tile, Enter open", "x": 10, "y": 180}],
         "buttons": [{"label": "Firewall", "x": 5, "y": 40, "w": 100, "h": 20}]}


class FakeDev(devui.Device):
    def __init__(self, out):
        super().__init__("fake", run_dir=out, client="/nonexistent", kind="qemu")
        self.calls = []

    def run(self, args, retries=None, timeout=None):
        self.calls.append(list(args))
        if args[:2] == ["state", "--json"]:
            return json.dumps(STATE)
        if args[0] == "screenshot":
            im = Image.new("RGB", (320, 200), (20, 30, 60))
            d = ImageDraw.Draw(im)
            for i in range(30):
                d.rectangle((i * 10, i * 5, i * 10 + 8, i * 5 + 20), fill=(200 - i * 5, 50 + i * 4, i * 8))
            im.save(args[2])
            return ""
        return ""


SCN = """# test scenario
row: F-T-1
targets: qemu
clause: 1 the page is drawn
feature-sha: %s
doc: a fake run
shot home
expect-not-blank
expect-text IYAKKI
""" % R.feature_sha(FEATURE)


@unittest.skipUnless(Image, "no Pillow")
class EndToEnd(unittest.TestCase):
    def setUp(self):  # the state text is the oracle here: no OCR of a drawn test picture
        old = devui.HAVE_TESSERACT
        devui.HAVE_TESSERACT = False
        self.addCleanup(lambda: setattr(devui, "HAVE_TESSERACT", old))

    def test_a_fake_run_writes_results_and_two_runs_raise_the_row(self):
        with tempfile.TemporaryDirectory() as tmp:
            scn_dir = os.path.join(tmp, "repo", "dist", "autotest", "scenarios")
            os.makedirs(scn_dir)
            with open(os.path.join(scn_dir, "F-T-1.scn"), "w") as fh:
                fh.write(SCN)
            run.ROOT = os.path.join(tmp, "repo")
            run.builtin_scenarios.load_text_scenarios(scn_dir)
            entries = [e for e in run.load_scenarios(scn_dir)]
            self.assertEqual([e["rows"] for e in entries], [["F-T-1"]])
            self.assertEqual(run.problems(entries, {"F-T-1"}), [])
            feats = {"F-T-1": FEATURE}
            for n, when in enumerate(("2026-11-21T10:00:00", "2026-11-22T02:30:00"), 1):
                out = os.path.join(tmp, "out", "run%d" % n)
                meta = {"run_id": "run%d" % n, "commit": "abc12345", "commit_source": "argument", "image": "img"}
                dev = FakeDev(out)
                recs = run.run_entries(entries, dev, "qemu", out, meta, feats, {}, False)
                self.assertEqual([r["status"] for r in recs], ["PASS"], recs)
                self.assertTrue(recs[0]["clauses_attested"])
                self.assertEqual(recs[0]["level"], "qemu")
                self.assertTrue(recs[0]["screen_evidence"])
                self.assertEqual(R.validate(recs[0]), [])
                # make the time of the second run later
                p = os.path.join(out, "F-T-1", "qemu", "result.json")
                r = json.load(open(p))
                r["ran_at"] = when
                json.dump(r, open(p, "w"))
                self.assertTrue(os.path.isfile(os.path.join(out, "F-T-1", "qemu", r["evidence"][0])))
            text = evidence.HEAD if hasattr(evidence, "HEAD") else ""
            features = ("| ID | Feature | Phase | Status | Pri | Impl/Test | Manual | Tests | Notes |\n|---|---|---|---|---|---|---|---|---|\n"
                        "| F-T-1 | %s | P5 | partial | P1 | 100/50 | none | `crate::a::b` | n |\n" % FEATURE)
            repo = os.path.join(tmp, "repo")
            new, rep = evidence.apply_to_text(features, os.path.join(tmp, "out"), datetime.datetime(2026, 11, 22, 12), repo, "abc12345")
            self.assertEqual(rep[0][1:3], (50, 70), rep)
            self.assertIn("100/70", new)

    def test_a_failing_check_is_a_fail_and_a_new_golden_is_not_a_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            scn_dir = os.path.join(tmp, "scn")
            os.makedirs(scn_dir)
            with open(os.path.join(scn_dir, "F-T-2.scn"), "w") as fh:
                fh.write("row: F-T-2\nclause: c\nshot home\nexpect-text NOT-ON-SCREEN-AT-ALL\n")
            with open(os.path.join(scn_dir, "F-T-3.scn"), "w") as fh:
                fh.write("row: F-T-3\nclause: c\nshot home\nexpect-golden home\n")
            entries = run.load_scenarios(scn_dir)
            meta = {"run_id": "r", "commit": "abc12345", "commit_source": "argument", "image": "img"}
            out = os.path.join(tmp, "out")
            old = verbs.Active.golden_root
            self.addCleanup(lambda: setattr(verbs.Active, "golden_root", old))
            recs = run.run_entries(entries, FakeDev(out), "qemu", out, meta, {}, {"golden_root": os.path.join(tmp, "golden")}, False)
            by = {r["row"]: r["status"] for r in recs}
            self.assertEqual(by, {"F-T-2": "FAIL", "F-T-3": "NEW-GOLDEN"})
            self.assertFalse(os.path.exists(os.path.join(tmp, "golden")), "a golden is written only by --update-golden")

    def test_the_guard_still_refuses_esc_on_home_inside_a_scenario(self):
        with tempfile.TemporaryDirectory() as tmp:
            scn_dir = os.path.join(tmp, "scn")
            os.makedirs(scn_dir)
            with open(os.path.join(scn_dir, "F-T-4.scn"), "w") as fh:
                fh.write("row: F-T-4\nclause: c\nkey esc\n")
            entries = run.load_scenarios(scn_dir)
            meta = {"run_id": "r", "commit": "abc12345", "commit_source": "argument", "image": "img"}
            recs = run.run_entries(entries, FakeDev(os.path.join(tmp, "o")), "qemu", os.path.join(tmp, "o"), meta, {}, {}, False)
            self.assertEqual(recs[0]["status"], "FAIL")
            self.assertIn("guard refused", recs[0]["reason"])


if __name__ == "__main__":
    unittest.main()
